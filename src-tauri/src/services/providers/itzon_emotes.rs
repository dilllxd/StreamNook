//! itzon's global and channel-specific 7TV emotes.
//!
//! The itzon channel response exposes the Twitch user id whose active 7TV set
//! the website loads. We mirror that contract, cache each channel independently,
//! and let the IRC adapter emit structured emote segments for both backlog and
//! live messages.

use crate::models::chat_layout::MessageSegment;
use crate::services::emote_service;
use crate::services::emote_service::{Emote, EmoteProvider, EmoteSet};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct ItzonEmote {
    pub id: String,
    pub url: String,
    pub zero_width: bool,
    pub width: Option<u32>,
    pub owner_name: Option<String>,
}

struct ChannelEmotes {
    map: HashMap<String, ItzonEmote>,
    fetched_at: Instant,
}

static STORE: OnceLock<Mutex<HashMap<String, ChannelEmotes>>> = OnceLock::new();
const TTL: Duration = Duration::from_secs(10 * 60);

fn store() -> &'static Mutex<HashMap<String, ChannelEmotes>> {
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn lookup(slug: &str, word: &str) -> Option<ItzonEmote> {
    store()
        .lock()
        .ok()?
        .get(&slug.to_lowercase())?
        .map
        .get(word)
        .cloned()
}

pub async fn channel_emote_set(slug: &str) -> EmoteSet {
    refresh(slug).await;
    let mut set = EmoteSet::new();
    if let Ok(cache) = store().lock() {
        if let Some(channel) = cache.get(&slug.to_lowercase()) {
            set.seven_tv = channel
                .map
                .iter()
                .map(|(name, emote)| Emote {
                    id: emote.id.clone(),
                    name: name.clone(),
                    url: emote.url.clone(),
                    provider: EmoteProvider::SevenTV,
                    is_zero_width: Some(emote.zero_width),
                    local_url: None,
                    emote_type: None,
                    owner_id: None,
                    owner_name: emote.owner_name.clone(),
                    modifier_flags: None,
                    ffz_sub_only: None,
                    width: emote.width,
                })
                .collect();
        }
    }
    set
}

/// Refresh global emotes first, then overlay the channel set so a channel emote
/// wins when it reuses a global name. A failed refresh never erases a good cache.
pub async fn refresh(slug: &str) {
    let slug = slug.trim().to_lowercase();
    if slug.is_empty() {
        return;
    }
    if store()
        .lock()
        .ok()
        .and_then(|cache| {
            cache
                .get(&slug)
                .map(|entry| entry.fetched_at.elapsed() < TTL)
        })
        .unwrap_or(false)
    {
        return;
    }

    let client = reqwest::Client::new();
    let mut map = HashMap::new();
    fetch_into(
        &client,
        "https://7tv.io/v3/emote-sets/global",
        "/emotes",
        &mut map,
    )
    .await;

    let twitch_id = match crate::services::providers::itzon_media::channel(&slug).await {
        Ok(channel) => channel.emote_twitch_id,
        Err(error) => {
            log::warn!("[itzon] failed to resolve 7TV id for '{slug}': {error}");
            None
        }
    };
    if let Some(twitch_id) = twitch_id.filter(|id| !id.is_empty()) {
        let user_url = format!("https://7tv.io/v3/users/twitch/{twitch_id}");
        if let Some(user) = fetch_json(&client, &user_url).await {
            if user
                .pointer("/emote_set/emotes")
                .and_then(Value::as_array)
                .is_some()
            {
                collect_emotes(&user, "/emote_set/emotes", &mut map);
            } else if let Some(set_id) = emote_service::seventv_active_set_id(&user) {
                fetch_into(
                    &client,
                    &format!("https://7tv.io/v3/emote-sets/{set_id}"),
                    "/emotes",
                    &mut map,
                )
                .await;
            }
        }
    }

    if map.is_empty() {
        log::warn!("[itzon] no 7TV emotes loaded for '{slug}'; keeping prior cache");
        return;
    }
    let count = map.len();
    if let Ok(mut cache) = store().lock() {
        cache.insert(
            slug.clone(),
            ChannelEmotes {
                map,
                fetched_at: Instant::now(),
            },
        );
    }
    log::info!("[itzon] 7TV emotes for '{slug}': {count} loaded");
}

pub fn parse_segments(slug: &str, text: &str) -> Vec<MessageSegment> {
    let mut segments = Vec::new();
    let mut buffer = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut cursor = 0;
    while cursor < chars.len() {
        if chars[cursor].is_whitespace() {
            buffer.push(chars[cursor]);
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < chars.len() && !chars[cursor].is_whitespace() {
            cursor += 1;
        }
        let word: String = chars[start..cursor].iter().collect();
        if let Some(emote) = lookup(slug, &word) {
            if !buffer.is_empty() {
                segments.push(MessageSegment::Text {
                    content: std::mem::take(&mut buffer),
                });
            }
            segments.push(MessageSegment::Emote {
                content: word,
                emote_id: Some(emote.id),
                emote_url: emote.url,
                is_zero_width: Some(emote.zero_width),
                modifier_flags: None,
                is_personal: None,
            });
        } else {
            buffer.push_str(&word);
        }
    }
    if !buffer.is_empty() || segments.is_empty() {
        segments.push(MessageSegment::Text { content: buffer });
    }
    segments
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

async fn fetch_into(
    client: &reqwest::Client,
    url: &str,
    pointer: &str,
    map: &mut HashMap<String, ItzonEmote>,
) {
    if let Some(payload) = fetch_json(client, url).await {
        collect_emotes(&payload, pointer, map);
    }
}

fn collect_emotes(payload: &Value, pointer: &str, map: &mut HashMap<String, ItzonEmote>) {
    let Some(items) = payload.pointer(pointer).and_then(Value::as_array) else {
        return;
    };
    for active in items {
        let data = active.get("data").unwrap_or(active);
        let Some(name) = active.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(id) = data
            .get("id")
            .or_else(|| active.get("id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let flags = data
            .get("flags")
            .or_else(|| active.get("flags"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let width = data
            .pointer("/host/files/0/width")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
        let owner_name = data
            .pointer("/owner/display_name")
            .and_then(Value::as_str)
            .map(String::from);
        map.insert(
            name.to_string(),
            ItzonEmote {
                id: id.to_string(),
                url: format!("https://cdn.7tv.app/emote/{id}/2x.webp"),
                zero_width: flags & 256 == 256,
                width,
                owner_name,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_current_active_emote_shape() {
        let payload = serde_json::json!({
            "emotes": [{
                "name": "Paint",
                "data": {
                    "id": "01TEST",
                    "flags": 256,
                    "host": { "files": [{ "width": 64 }] },
                    "owner": { "display_name": "Artist" }
                }
            }]
        });
        let mut map = HashMap::new();
        collect_emotes(&payload, "/emotes", &mut map);
        let emote = map.get("Paint").unwrap();
        assert_eq!(emote.id, "01TEST");
        assert!(emote.zero_width);
        assert_eq!(emote.width, Some(64));
        assert_eq!(emote.owner_name.as_deref(), Some("Artist"));
    }

    #[test]
    fn parser_preserves_text_when_cache_is_empty() {
        let segments = parse_segments("missing-channel", "hello  world");
        assert_eq!(segments.len(), 1);
        assert!(matches!(
            &segments[0],
            MessageSegment::Text { content } if content == "hello  world"
        ));
    }

    #[test]
    fn parser_keeps_channel_caches_isolated() {
        let emote = |id: &str| ItzonEmote {
            id: id.into(),
            url: format!("https://cdn.7tv.app/emote/{id}/2x.webp"),
            zero_width: false,
            width: Some(32),
            owner_name: None,
        };
        let mut cache = store().lock().unwrap();
        cache.insert(
            "room-one".into(),
            ChannelEmotes {
                map: HashMap::from([("OnlyOne".into(), emote("one"))]),
                fetched_at: Instant::now(),
            },
        );
        cache.insert(
            "room-two".into(),
            ChannelEmotes {
                map: HashMap::from([("OnlyTwo".into(), emote("two"))]),
                fetched_at: Instant::now(),
            },
        );
        drop(cache);

        let one = parse_segments("room-one", "OnlyOne OnlyTwo");
        let two = parse_segments("room-two", "OnlyOne OnlyTwo");
        assert!(matches!(&one[0], MessageSegment::Emote { content, .. } if content == "OnlyOne"));
        assert!(matches!(&two[1], MessageSegment::Emote { content, .. } if content == "OnlyTwo"));
    }

    #[tokio::test]
    #[ignore = "live itzon + 7TV protocol smoke test"]
    async fn live_channel_loads_and_tokenizes_its_current_set() {
        let set = channel_emote_set("steqian").await;
        assert!(set.seven_tv.len() > 100);
        let sample = set.seven_tv.first().unwrap();
        let parsed = parse_segments("steqian", &sample.name);
        assert!(
            matches!(&parsed[0], MessageSegment::Emote { emote_id, .. } if emote_id.as_deref() == Some(sample.id.as_str()))
        );
    }
}
