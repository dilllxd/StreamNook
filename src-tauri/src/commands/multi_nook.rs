use crate::models::settings::AppState;
use crate::services::itzon_media;
use crate::services::multi_nook_server::MultiNookServer;
use crate::services::twitch_resolver as tr;
use log::debug;
use tauri::State;

/// Maximum number of concurrent streams allowed
const MAX_STREAMS: usize = 25;

/// Extract the channel login from a twitch.tv live URL. MultiNook tiles are
/// always live channels, so this is enough.
fn channel_from_url(url: &str) -> Option<String> {
    let after = url.split("twitch.tv/").nth(1)?;
    let seg = after.split(['/', '?', '#']).next()?.trim();
    if seg.is_empty() || seg == "videos" || seg == "directory" {
        return None;
    }
    Some(seg.to_lowercase())
}

fn itzon_channel_from_url(url: &str) -> Option<String> {
    let after = url.split("itzon.tv/").nth(1)?;
    let seg = after.split(['/', '?', '#']).next()?.trim();
    if seg.is_empty() {
        return None;
    }
    Some(seg.to_lowercase())
}

/// Start a stream for multi-stream mode. Each tile resolves natively (same
/// pipeline as the solo player) and gets its own proxy server.
#[tauri::command]
pub async fn start_multi_nook(
    stream_id: String,
    url: String,
    quality: String,
    provider: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let provider = provider.unwrap_or_else(|| "twitch".to_string());
    debug!(
        "[MultiNook] start_multi_nook called: id='{}', provider='{}', url='{}', quality='{}'",
        stream_id, provider, url, quality
    );

    let current_count = MultiNookServer::active_count().await;
    if current_count >= MAX_STREAMS {
        return Err(format!(
            "Maximum of {} concurrent streams reached",
            MAX_STREAMS
        ));
    }

    itzon_media::stop_heartbeat(&stream_id).await;

    if provider == "itzon" {
        let channel = itzon_channel_from_url(&url)
            .ok_or_else(|| format!("Unrecognized itzon URL: {}", url))?;
        let playback = itzon_media::resolve_hls(&channel)
            .await
            .map_err(|error| error.to_string())?;
        let port = MultiNookServer::start_proxy(&stream_id, playback.hls_url.clone())
            .await
            .map_err(|error| error.to_string())?;
        itzon_media::start_heartbeat(&stream_id, &playback).await;

        let proxy_url = format!(
            "http://localhost:{}/stream.m3u8?t={}",
            port,
            chrono::Utc::now().timestamp_millis()
        );
        debug!(
            "[MultiNook] '{}' itzon:{} → {}",
            stream_id, playback.channel, proxy_url
        );
        return Ok(proxy_url);
    }

    if provider != "twitch" {
        return Err(format!("Playback provider '{}' is not supported", provider));
    }

    let stream_timeout = { state.settings.lock().unwrap().streamlink.stream_timeout };

    let channel =
        channel_from_url(&url).ok_or_else(|| format!("Unrecognized Twitch URL: {}", url))?;
    let oauth = state.twitch_auth.get_token().await.ok();

    // MultiNook resolves each tile with a SINGLE attempt (retry_delay = 0). Unlike
    // the solo player, a grid tile is expected to be live, so the solo path's
    // retry-until-live loop is wrong here: it would keep an offline channel
    // hammering usher / GQL every `retry_streams` seconds for the full
    // `stream_timeout` budget (60s by default), saturating the network and
    // stalling the OTHER tiles' playback. Failing fast lets an offline tile show
    // its overlay right away; the per-tile Retry button (frontend) covers the
    // rare "channel just went live" case. `stream_timeout` is still passed as
    // the budget but is moot at retry_delay = 0 (single attempt).
    let core =
        tr::resolve_live_resilient(&channel, oauth.as_deref(), &quality, 0, stream_timeout).await;

    // Same hand-off as the solo player: a resolution-owning plugin takes the
    // non-entitled tile when installed, addressed by this tile's stream id.
    let r = match crate::commands::streaming::resolve_via_plugin(
        &state, &stream_id, &channel, &quality, &core,
    )
    .await
    {
        Some(plugin_resolved) => plugin_resolved,
        None => core.map_err(|e| e.to_string())?,
    };

    let port = MultiNookServer::start_proxy(&stream_id, r.url)
        .await
        .map_err(|e| e.to_string())?;

    // Tag the proxy URL when the tile's relay activated its LL-HLS origin (settled
    // inside start_proxy, before this point). The player must choose its hls.js mode
    // at construction, and riding the flag on the URL it already consumes keeps the
    // two atomic: a refreshed URL always carries the matching mode.
    let low_latency = MultiNookServer::is_low_latency(&stream_id).await;
    let proxy_url = format!(
        "http://localhost:{}/stream.m3u8?t={}{}",
        port,
        chrono::Utc::now().timestamp_millis(),
        if low_latency { "&ll=1" } else { "" }
    );

    debug!(
        "[MultiNook] '{}' ({}) → {} (mode={})",
        stream_id, channel, proxy_url, r.status.mode
    );

    Ok(proxy_url)
}

/// Stop a specific stream in multi-stream mode
#[tauri::command]
pub async fn stop_multi_nook(stream_id: String) -> Result<(), String> {
    debug!("[MultiNook] Stopping stream: {}", stream_id);
    itzon_media::stop_heartbeat(&stream_id).await;
    MultiNookServer::stop_instance(&stream_id)
        .await
        .map_err(|e| e.to_string())
}

/// Stop all streams in multi-stream mode (cleanup)
#[tauri::command]
pub async fn stop_all_multi_nooks() -> Result<(), String> {
    debug!("[MultiNook] Stopping all multi-stream instances");
    itzon_media::stop_all_heartbeats_except(crate::services::stream_server::SOLO_STREAM_ID).await;
    MultiNookServer::stop_all().await.map_err(|e| e.to_string())
}

/// Get a list of active multi-stream IDs
#[tauri::command]
pub async fn get_active_multi_nooks() -> Result<Vec<String>, String> {
    Ok(MultiNookServer::get_active_streams().await)
}
