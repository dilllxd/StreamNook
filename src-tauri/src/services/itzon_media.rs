use anyhow::{anyhow, Context, Result};
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
