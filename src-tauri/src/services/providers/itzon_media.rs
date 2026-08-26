use super::key;
use super::source::{PlaybackKind, PlaybackQuality, ResolvedPlayback, SourceCaps, StreamSource};
use crate::models::provider_stream::{CategoryPage, ProviderCategory, ProviderStream, StreamPage};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use rand::Rng;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const ITZON_ORIGIN: &str = "https://itzon.tv";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(15))
        .build()
        .expect("failed to build itzon HTTP client")
});

static HEARTBEATS: Lazy<Mutex<HashMap<String, JoinHandle<()>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static RESOLVED_PLAYBACK: Lazy<Mutex<HashMap<String, ItzonPlayback>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItzonStream {
    pub username: String,
    #[serde(default)]
    pub viewers: u64,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub hls_base: Option<String>,
    #[serde(default)]
    pub media_base: Option<String>,
    #[serde(default)]
    pub wss_base: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub category_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItzonCategory {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub live_stream_count: u64,
    #[serde(default)]
    pub viewer_count: u64,
    #[serde(default)]
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItzonExplore {
    #[serde(default)]
    pub streams: Vec<ItzonStream>,
    #[serde(default)]
    pub categories: Vec<ItzonCategory>,
    #[serde(default)]
    pub hls_base: Option<String>,
    #[serde(default)]
    pub media_base: Option<String>,
    #[serde(default)]
    pub wss_base: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItzonChannel {
    pub username: String,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub partner: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub category_id: Option<u64>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub points_name: Option<String>,
    #[serde(default)]
    pub emote_twitch_id: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub hls_base: Option<String>,
    #[serde(default)]
    pub media_base: Option<String>,
    #[serde(default)]
    pub wss_base: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ItzonPlayback {
    pub channel: String,
    pub hls_url: String,
    pub heartbeat_url: String,
    pub viewer_id: String,
}

pub async fn explore() -> Result<ItzonExplore> {
    HTTP_CLIENT
        .get(format!("{ITZON_ORIGIN}/api/live/explore"))
        .send()
        .await
        .context("itzon explore request failed")?
        .error_for_status()
        .context("itzon explore returned an error")?
        .json::<ItzonExplore>()
        .await
        .context("invalid itzon explore response")
}

pub async fn channel(username: &str) -> Result<ItzonChannel> {
    let username = normalize_username(username)?;
    HTTP_CLIENT
        .get(format!(
            "{ITZON_ORIGIN}/api/live/channel/{}",
            urlencoding::encode(&username)
        ))
        .send()
        .await
        .context("itzon channel request failed")?
        .error_for_status()
        .context("itzon channel returned an error")?
        .json::<ItzonChannel>()
        .await
        .context("invalid itzon channel response")
}

pub async fn resolve_hls(username: &str) -> Result<ItzonPlayback> {
    let requested = normalize_username(username)?;
    let channel = channel(&requested).await?;
    if channel.locked {
        return Err(anyhow!("itzon channel '{}' is private", requested));
    }
    if !channel.live {
        return Err(anyhow!("itzon channel '{}' is offline", requested));
    }

    let hls_base = channel
        .hls_base
        .as_deref()
        .ok_or_else(|| anyhow!("itzon did not return an HLS edge for '{}'", requested))?;
    let hls_base = validate_itzon_base(hls_base)?;
    let channel_key = normalize_username(&channel.username).unwrap_or(requested);
    let encoded = urlencoding::encode(&channel_key).into_owned();
    let viewer_id = format!("{:016x}", rand::rng().random::<u64>());

    Ok(ItzonPlayback {
        channel: channel_key,
        hls_url: format!("{hls_base}/hls/{encoded}/live.m3u8"),
        heartbeat_url: format!("{hls_base}/hls/{encoded}/beat?id={viewer_id}"),
        viewer_id,
    })
}

pub async fn start_heartbeat(stream_id: &str, playback: &ItzonPlayback) {
    stop_heartbeat(stream_id).await;

    let id = stream_id.to_string();
    let channel = playback.channel.clone();
    let url = playback.heartbeat_url.clone();
    let viewer_id = playback.viewer_id.clone();
    let client = HTTP_CLIENT.clone();
    let handle = tokio::spawn(async move {
        let mut timer = tokio::time::interval(HEARTBEAT_INTERVAL);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if let Err(error) = client
                .post(&url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                log::debug!(
                    "[itzon] viewer heartbeat failed for '{}' (viewer {}): {}",
                    channel,
                    viewer_id,
                    error
                );
            }
        }
    });

    HEARTBEATS.lock().await.insert(id, handle);
}

pub async fn stop_heartbeat(stream_id: &str) {
    if let Some(handle) = HEARTBEATS.lock().await.remove(stream_id) {
        handle.abort();
    }
}

pub async fn stop_all_heartbeats_except(except: &str) {
    let mut heartbeats = HEARTBEATS.lock().await;
    let stopped = heartbeats
        .keys()
        .filter(|key| key.as_str() != except)
        .cloned()
        .collect::<Vec<_>>();
    for key in stopped {
        let Some(handle) = heartbeats.remove(&key) else {
            continue;
        };
        handle.abort();
    }
}

pub async fn take_resolved_playback(channel: &str) -> Option<ItzonPlayback> {
    let channel = normalize_username(channel).ok()?;
    RESOLVED_PLAYBACK.lock().await.remove(&channel)
}

pub struct ItzonSource;

impl ItzonSource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl StreamSource for ItzonSource {
    fn id(&self) -> &'static str {
        "itzon"
    }

    fn caps(&self) -> SourceCaps {
        SourceCaps {
            playback: true,
            directory: true,
            search: true,
            native_follows: true,
            live_check: true,
        }
    }

    async fn resolve_playback(&self, channel: &str, _quality: &str) -> Result<ResolvedPlayback> {
        let playback = resolve_hls(channel).await?;
        let url = playback.hls_url.clone();
        RESOLVED_PLAYBACK
            .lock()
            .await
            .insert(playback.channel.clone(), playback);
        Ok(ResolvedPlayback {
            kind: PlaybackKind::Hls,
            url: url.clone(),
            quality: "best".to_string(),
            qualities: vec![PlaybackQuality {
                name: "best".to_string(),
                url,
                width: None,
                height: None,
                fps: None,
                bandwidth: None,
            }],
        })
    }

    async fn channel_meta(&self, channel_name: &str) -> Result<ProviderStream> {
        Ok(row_from_channel(channel(channel_name).await?))
    }

    async fn directory(
        &self,
        category: Option<&str>,
        _cursor: Option<&str>,
        limit: u32,
    ) -> Result<StreamPage> {
        let data = explore().await?;
        let media_base = data.media_base.as_deref();
        let streams = data
            .streams
            .into_iter()
            .filter(|stream| {
                category.is_none_or(|wanted| {
                    stream.category_id.map(|id| id.to_string()).as_deref() == Some(wanted)
                        || stream
                            .category
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
                })
            })
            .take(limit.clamp(1, 100) as usize)
            .map(|stream| row_from_stream(stream, media_base))
            .collect();
        Ok(StreamPage {
            streams,
            cursor: None,
        })
    }

    async fn search(&self, query: &str) -> Result<StreamPage> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(StreamPage {
                streams: vec![],
                cursor: None,
            });
        }
        let needle = query.to_lowercase();
        let data = explore().await?;
        let media_base = data.media_base.as_deref();
        let mut streams: Vec<ProviderStream> = data
            .streams
            .into_iter()
            .filter(|stream| {
                stream.username.to_lowercase().contains(&needle)
                    || stream
                        .title
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&needle)
                    || stream
                        .category
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&needle)
            })
            .map(|stream| row_from_stream(stream, media_base))
            .collect();
        if streams.is_empty() {
            if let Ok(found) = channel(query).await {
                streams.push(row_from_channel(found));
            }
        }
        Ok(StreamPage {
            streams,
            cursor: None,
        })
    }

    async fn categories(&self, _cursor: Option<&str>, limit: u32) -> Result<CategoryPage> {
        let mut categories: Vec<ProviderCategory> = explore()
            .await?
            .categories
            .into_iter()
            .map(|category| ProviderCategory {
                provider: "itzon".to_string(),
                id: category.id.to_string(),
                name: category.name,
                thumbnail: category
                    .image_url
                    .as_deref()
                    .and_then(trusted_asset_url)
                    .unwrap_or_default(),
                viewer_count: count(category.viewer_count),
                channel_count: count(category.live_stream_count),
            })
            .collect();
        categories.sort_by(|a, b| b.viewer_count.cmp(&a.viewer_count));
        categories.truncate(limit.clamp(1, 100) as usize);
        Ok(CategoryPage {
            categories,
            cursor: None,
        })
    }

    async fn live_check(&self, channels: &[String]) -> Result<Vec<ProviderStream>> {
        let wanted: std::collections::HashSet<String> = channels
            .iter()
            .filter_map(|channel| normalize_username(channel).ok())
            .collect();
        if wanted.is_empty() {
            return Ok(vec![]);
        }
        let data = explore().await?;
        let media_base = data.media_base.as_deref();
        Ok(data
            .streams
            .into_iter()
            .filter(|stream| wanted.contains(&stream.username.to_lowercase()))
            .map(|stream| row_from_stream(stream, media_base))
            .collect())
    }

    async fn followed_live(&self) -> Result<Vec<ProviderStream>> {
        let followed = crate::services::itzon_auth_service::followed_usernames().await?;
        self.live_check(&followed).await
    }
}

fn row_from_stream(stream: ItzonStream, explore_media_base: Option<&str>) -> ProviderStream {
    let username = stream.username.trim().to_lowercase();
    let display_name = stream.username.trim().to_string();
    let media_base = stream.media_base.as_deref().or(explore_media_base);
    ProviderStream {
        provider: "itzon".to_string(),
        key: key::make_key("itzon", &username),
        id: key::make_key("itzon", &username),
        user_id: key::make_key("itzon", &username),
        user_login: username.clone(),
        user_name: display_name.clone(),
        title: stream
            .title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| format!("{} live on itzon", display_name)),
        viewer_count: count(stream.viewers),
        game_id: stream
            .category_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        game_name: stream
            .category
            .unwrap_or_else(|| "Live on itzon".to_string()),
        category_thumbnail: None,
        thumbnail_url: thumbnail_url(&username, media_base, stream.thumbnail.as_deref()),
        started_at: String::new(),
        profile_image_url: Some(avatar_url(&username)),
        is_live: true,
        watch_url: format!("https://itzon.tv/{}", urlencoding::encode(&username)),
        tags: stream.language.map(|language| vec![language]),
    }
}

fn row_from_channel(channel: ItzonChannel) -> ProviderStream {
    let username = channel.username.trim().to_lowercase();
    let display_name = channel.username.trim().to_string();
    ProviderStream {
        provider: "itzon".to_string(),
        key: key::make_key("itzon", &username),
        id: key::make_key("itzon", &username),
        user_id: key::make_key("itzon", &username),
        user_login: username.clone(),
        user_name: display_name.clone(),
        title: channel
            .title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| format!("{} live on itzon", display_name)),
        viewer_count: 0,
        game_id: channel
            .category_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        game_name: channel
            .category
            .unwrap_or_else(|| "Live on itzon".to_string()),
        category_thumbnail: None,
        thumbnail_url: thumbnail_url(
            &username,
            channel.media_base.as_deref(),
            channel.thumbnail.as_deref(),
        ),
        started_at: String::new(),
        profile_image_url: Some(avatar_url(&username)),
        is_live: channel.live,
        watch_url: format!("https://itzon.tv/{}", urlencoding::encode(&username)),
        tags: channel.language.map(|language| vec![language]),
    }
}

fn avatar_url(username: &str) -> String {
    format!(
        "{ITZON_ORIGIN}/api/live/profile/{}/avatar",
        urlencoding::encode(username)
    )
}

fn thumbnail_url(username: &str, media_base: Option<&str>, custom: Option<&str>) -> String {
    if let Some(url) = custom.and_then(trusted_asset_url) {
        return url;
    }
    if let Some(base) = media_base.and_then(trusted_asset_url) {
        return format!(
            "{}/thumb/{}.jpg?t={}",
            base.trim_end_matches('/'),
            urlencoding::encode(username),
            chrono::Utc::now().timestamp() / 60
        );
    }
    avatar_url(username)
}

fn trusted_asset_url(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    let host = parsed.host_str()?;
    (parsed.scheme() == "https" && (host == "itzon.tv" || host.ends_with(".itzon.tv")))
        .then(|| parsed.to_string())
}

fn count(value: u64) -> u32 {
    value.min(u32::MAX as u64) as u32
}

fn normalize_username(username: &str) -> Result<String> {
    let username = username.trim().to_lowercase();
    if username.is_empty()
        || username.len() > 64
        || !username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(anyhow!("invalid itzon channel name"));
    }
    Ok(username)
}

fn validate_itzon_base(base: &str) -> Result<String> {
    let parsed = Url::parse(base).context("itzon returned an invalid HLS edge")?;
    if parsed.scheme() != "https" {
        return Err(anyhow!("itzon HLS edge must use HTTPS"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("itzon HLS edge has no host"))?;
    if host != "itzon.tv" && !host.ends_with(".itzon.tv") {
        return Err(anyhow!("itzon returned an untrusted HLS edge"));
    }
    Ok(base.trim_end_matches('/').to_string())
}

pub fn is_itzon_hls_url(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| host == "itzon.tv" || host.ends_with(".itzon.tv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_channel_names() {
        assert_eq!(normalize_username(" Zeenote ").unwrap(), "zeenote");
        assert!(normalize_username("bad/name").is_err());
    }

    #[test]
    fn accepts_only_itzon_hls_edges() {
        assert_eq!(
            validate_itzon_base("https://de-hls.itzon.tv/").unwrap(),
            "https://de-hls.itzon.tv"
        );
        assert!(validate_itzon_base("http://de-hls.itzon.tv").is_err());
        assert!(validate_itzon_base("https://itzon.tv.evil.example").is_err());
    }

    #[test]
    fn recognizes_only_itzon_hls_hosts() {
        assert!(is_itzon_hls_url(
            "https://de-hls.itzon.tv/hls/channel/live.m3u8"
        ));
        assert!(is_itzon_hls_url("https://itzon.tv/hls/live.m3u8"));
        assert!(!is_itzon_hls_url(
            "https://itzon.tv.evil.example/hls/live.m3u8"
        ));
        assert!(!is_itzon_hls_url("not a URL"));
    }
}
