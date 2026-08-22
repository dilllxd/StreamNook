//! Tauri commands for moderator rooms.
//!
//! Two responsibilities: the one-time scoped consent that mints a
//! `user:read:moderated_channels` token, and the per-channel room-token request
//! that trades that token (plus the user's entitlement) for a short-lived room
//! token from the gate Worker. The frontend opens the room WebSocket with the
//! returned room token; the scoped Twitch token never leaves the desktop.

use crate::services::modroom_auth_service as auth;
use serde::Serialize;
use tauri::AppHandle;

/// Gate Worker base (custom domain on the streamnook.app zone).
const MODROOM_API_BASE: &str = "https://modroom.streamnook.app";

#[derive(Serialize)]
pub struct ModRoomStatus {
    pub connected: bool,
    pub login: Option<String>,
}

/// Whether the scoped consent has been granted, and as whom.
#[tauri::command]
pub async fn modroom_status() -> Result<ModRoomStatus, String> {
    Ok(ModRoomStatus {
        connected: auth::is_connected(),
        login: auth::connected_login(),
    })
}

/// Run the one-time scoped consent in the system browser and store the token.
#[tauri::command]
pub async fn modroom_connect(app: AppHandle) -> Result<ModRoomStatus, String> {
    use tauri_plugin_opener::OpenerExt;

    let flow = auth::start_device_authorization()
        .await
        .map_err(|e| e.to_string())?;
    app.opener()
        .open_url(flow.verification_uri, None::<String>)
        .map_err(|e| format!("Failed to open browser: {}", e))?;

    let cred =
        auth::complete_device_authorization(&flow.device_code, flow.interval, flow.expires_in)
            .await
            .map_err(|e| e.to_string())?;
    Ok(ModRoomStatus {
        connected: true,
        login: Some(cred.login),
    })
}

/// Forget the scoped credential.
#[tauri::command]
pub async fn modroom_disconnect() -> Result<(), String> {
    auth::disconnect().map_err(|e| e.to_string())
}

/// Broadcaster ids of every channel this account moderates. Empty if not
/// connected; the UI then falls back to per-channel detection.
#[tauri::command]
pub async fn modroom_list_moderated() -> Result<Vec<String>, String> {
    Ok(auth::list_moderated_channels().await.unwrap_or_default())
}

#[derive(Serialize)]
pub struct RoomToken {
    pub token: String,
    pub role: String,
    pub channel_id: String,
    pub expires_at: i64,
    pub ttl: i64,
    /// Per-channel encryption key (base64), delivered only to verified mods.
    pub room_key: String,
    /// The caller's own Twitch user id (to identify their own messages).
    pub user_id: String,
    /// The caller's own login (optimistic rendering + mention detection).
    pub login: String,
}

/// Request a room token for a channel. Returns the gate's error string on denial
/// (`not_moderator`, `not_entitled`, ...) or `needs_connect` if the scoped token
/// is missing or unrefreshable, so the UI can prompt the consent.
#[tauri::command]
pub async fn modroom_get_room_token(channel_id: String) -> Result<RoomToken, String> {
    if channel_id.trim().is_empty() {
        return Err("missing_channel".to_string());
    }

    // `network` (unlike `needs_connect`) tells the frontend this is transient:
    // it retries with backoff instead of showing the consent CTA to a user who
    // is already connected but briefly offline.
    let token = auth::get_valid_access_token().await.map_err(|e| match e {
        auth::TokenFail::NeedsConnect => "needs_connect".to_string(),
        auth::TokenFail::Network => "network".to_string(),
    })?;

    let client = crate::services::http::client().clone();
    let resp = client
        .post(format!("{}/token", MODROOM_API_BASE))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "channelId": channel_id }))
        .send()
        .await
        .map_err(|_| "network".to_string())?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|_| "network".to_string())?;

    if !status.is_success() {
        let err = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("request_failed");
        return Err(err.to_string());
    }

    Ok(RoomToken {
        token: body
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        role: body
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        channel_id: body
            .get("channelId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        expires_at: body.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or_default(),
        ttl: body.get("ttl").and_then(|v| v.as_i64()).unwrap_or_default(),
        room_key: body
            .get("roomKey")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        user_id: body
            .get("userId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        login: body
            .get("login")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}
