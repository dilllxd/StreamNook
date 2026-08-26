//! Itzon account authentication.
//!
//! Registered builds use the documented public-client OAuth PKCE flow. Until the
//! fork has its own client ID, the existing site-owned WebView session remains a
//! compatibility fallback for chat sending and the website-only following API.

use crate::services::twitch_service::get_app_data_dir;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const ORIGIN: &str = "https://itzon.tv";
const SESSION_URL: &str = "https://itzon.tv/api/main/v1/auth/session";
const FOLLOWS_URL: &str = "https://itzon.tv/api/main/v1/follows/mine";
const WS_CONFIG_URL: &str = "https://itzon.tv/api/live/ws-config";
const LOGIN_WINDOW_LABEL: &str = "itzon-login";
const RESTORE_WINDOW_LABEL: &str = "itzon-session-restore";
const VALIDATION_TTL: Duration = Duration::from_secs(4 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct ItzonSession {
    origin_cookies: HashMap<String, String>,
    chat_cookies: HashMap<String, String>,
    username: String,
    token: String,
    validated_at: Instant,
}

pub struct ChatAuth {
    pub cookie_header: Option<String>,
    pub pass: Option<String>,
    pub nick: Option<String>,
}

#[derive(Deserialize)]
struct SessionInfo {
    kind: String,
    username: String,
    #[serde(default)]
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsConfig {
    chat_wss: String,
}

#[derive(Deserialize)]
struct FollowingResponse {
    #[serde(default)]
    following: Vec<String>,
}

#[derive(Default)]
struct CookieDelta {
    shared_upserts: HashMap<String, String>,
    shared_deletes: HashSet<String>,
}

static SESSION: OnceLock<Mutex<Option<ItzonSession>>> = OnceLock::new();
static RESTORE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static REVISION: AtomicU64 = AtomicU64::new(0);

fn session_cell() -> &'static Mutex<Option<ItzonSession>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

pub fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

fn bump_revision() {
    REVISION.fetch_add(1, Ordering::SeqCst);
}

pub(crate) fn oauth_changed() {
    bump_revision();
}

pub fn oauth_configured() -> bool {
    crate::services::itzon_oauth_service::is_configured()
}

pub fn auth_method() -> &'static str {
    if crate::services::itzon_oauth_service::has_account() {
        "oauth"
    } else if session_cell()
        .lock()
        .map(|session| session.is_some())
        .unwrap_or(false)
    {
        "website-session"
    } else if oauth_configured() {
        "oauth-ready"
    } else {
        "registration-pending"
    }
}

pub fn is_connected() -> bool {
    crate::services::itzon_oauth_service::has_account()
        || session_cell()
            .lock()
            .map(|session| session.is_some())
            .unwrap_or(false)
}

pub fn account_name() -> Option<String> {
    if let Some(username) = crate::services::itzon_oauth_service::account_name() {
        return Some(username);
    }
    session_cell()
        .lock()
        .ok()
        .and_then(|session| session.as_ref().map(|session| session.username.clone()))
}

pub async fn chat_auth() -> ChatAuth {
    match crate::services::itzon_oauth_service::chat_credential().await {
        Ok(Some(credential)) => {
            return ChatAuth {
                cookie_header: None,
                pass: Some(credential.access_token),
                nick: Some(credential.username),
            };
        }
        Ok(None) => {}
        Err(error) => log::warn!("[itzon] OAuth credential unavailable: {error}"),
    }
    if ensure_cookie_valid().await {
        return ChatAuth {
            cookie_header: chat_cookie_header(),
            pass: None,
            nick: None,
        };
    }
    ChatAuth {
        cookie_header: None,
        pass: None,
        nick: None,
    }
}

pub fn chat_cookie_header() -> Option<String> {
    session_cell()
        .lock()
        .ok()
        .and_then(|session| {
            session
                .as_ref()
                .map(|session| cookie_header(&session.chat_cookies))
        })
        .filter(|header| !header.is_empty())
}

pub async fn ensure_valid() -> bool {
    match crate::services::itzon_oauth_service::chat_credential().await {
        Ok(Some(_)) => return true,
        Ok(None) => {}
        Err(error) => log::warn!("[itzon] OAuth validation failed: {error}"),
    }
    ensure_cookie_valid().await
}

async fn ensure_cookie_valid() -> bool {
    let snapshot_revision = revision();
    let snapshot = session_cell()
        .lock()
        .ok()
        .and_then(|session| session.clone());
    let Some(snapshot) = snapshot else {
        return false;
    };
    if snapshot.validated_at.elapsed() < VALIDATION_TTL {
        return true;
    }

    match validate_session(snapshot.origin_cookies.clone()).await {
        Ok((info, origin_cookies, chat_delta)) => {
            if let Ok(mut session) = session_cell().lock() {
                if revision() != snapshot_revision {
                    return session.is_some();
                }
                if let Some(current) = session.as_mut() {
                    current.username = info.username;
                    current.token = info.token;
                    current.origin_cookies = origin_cookies;
                    for name in chat_delta.shared_deletes {
                        current.chat_cookies.remove(&name);
                    }
                    for (name, value) in chat_delta.shared_upserts {
                        current.chat_cookies.insert(name, value);
                    }
                    current.validated_at = Instant::now();
                    return true;
                }
            }
            false
        }
        Err(error) => {
            log::warn!("[itzon] session expired: {error}");
            if clear_memory_session_if_revision(snapshot_revision) {
                false
            } else {
                is_connected()
            }
        }
    }
}

pub async fn followed_usernames() -> Result<Vec<String>> {
    if let Some(credential) = crate::services::itzon_oauth_service::chat_credential().await? {
        let response = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()?
            .get(FOLLOWS_URL)
            .bearer_auth(credential.access_token)
            .send()
            .await
            .context("request itzon follows with OAuth")?
            .error_for_status()
            .context("itzon OAuth follows response")?
            .json::<FollowingResponse>()
            .await
            .context("decode itzon OAuth follows")?;
        return Ok(normalize_following(response.following));
    }

    let has_cookie_session = session_cell()
        .lock()
        .map(|session| session.is_some())
        .unwrap_or(false);
    if !has_cookie_session && !restore_cookie().await {
        return Ok(Vec::new());
    }
    if !ensure_cookie_valid().await {
        return Ok(Vec::new());
    }

    let snapshot_revision = revision();
    let mut cookies = session_cell()
        .lock()
        .ok()
        .and_then(|session| {
            session
                .as_ref()
                .map(|session| session.origin_cookies.clone())
        })
        .ok_or_else(|| anyhow!("itzon session unavailable"))?;

    let response = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()?
        .get(FOLLOWS_URL)
        .header(reqwest::header::COOKIE, cookie_header(&cookies))
        .send()
        .await
        .context("itzon follows request failed")?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        clear_memory_session_if_revision(snapshot_revision);
        return Ok(Vec::new());
    }
    let response = response
        .error_for_status()
        .context("itzon follows returned an error")?;
    let chat_delta = merge_response_cookies(response.headers(), &mut cookies);
    let following = response
        .json::<FollowingResponse>()
        .await
        .context("invalid itzon follows response")?;

    if let Ok(mut session) = session_cell().lock() {
        if revision() == snapshot_revision {
            if let Some(current) = session.as_mut() {
                current.origin_cookies = cookies;
                for name in chat_delta.shared_deletes {
                    current.chat_cookies.remove(&name);
                }
                for (name, value) in chat_delta.shared_upserts {
                    current.chat_cookies.insert(name, value);
                }
            }
        }
    }

    Ok(normalize_following(following.following))
}

fn normalize_following(usernames: Vec<String>) -> Vec<String> {
    let mut usernames = usernames
        .into_iter()
        .map(|username| username.trim().to_lowercase())
        .filter(|username| {
            !username.is_empty()
                && username.len() <= 64
                && username
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .collect::<Vec<_>>();
    usernames.sort();
    usernames.dedup();
    usernames
}

fn clear_memory_session_if_revision(expected_revision: u64) -> bool {
    let Ok(mut session) = session_cell().lock() else {
        return false;
    };
    if revision() != expected_revision {
        return false;
    }
    let changed = session.take().is_some();
    if changed {
        bump_revision();
    }
    changed
}

fn profile_dir() -> PathBuf {
    let base = get_app_data_dir().unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("platform_web_profiles").join("itzon");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn cookie_header(cookies: &HashMap<String, String>) -> String {
    let mut entries: Vec<_> = cookies.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn validate_session(
    mut cookies: HashMap<String, String>,
) -> Result<(SessionInfo, HashMap<String, String>, CookieDelta)> {
    let response = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()?
        .get(SESSION_URL)
        .header(reqwest::header::COOKIE, cookie_header(&cookies))
        .send()
        .await
        .context("itzon session check failed")?;
    if !response.status().is_success() {
        return Err(anyhow!("itzon session returned HTTP {}", response.status()));
    }
    let chat_delta = merge_response_cookies(response.headers(), &mut cookies);
    let info = response
        .json::<SessionInfo>()
        .await
        .context("invalid itzon session response")?;
    if !matches!(info.kind.as_str(), "user" | "admin") || info.username.trim().is_empty() {
        return Err(anyhow!("itzon did not confirm a user account"));
    }
    Ok((info, cookies, chat_delta))
}

/// Merge renewed origin cookies and return only cookies explicitly scoped to an
/// itzon domain, which are also valid for the separate chat edge host.
fn merge_response_cookies(
    headers: &reqwest::header::HeaderMap,
    origin: &mut HashMap<String, String>,
) -> CookieDelta {
    let mut delta = CookieDelta::default();
    for value in headers.get_all(reqwest::header::SET_COOKIE) {
        let Ok(raw) = value.to_str() else {
            continue;
        };
        let mut parts = raw.split(';');
        let Some((name, value)) = parts.next().and_then(|pair| pair.split_once('=')) else {
            continue;
        };
        let attrs: Vec<String> = parts.map(|part| part.trim().to_lowercase()).collect();
        let shared = attrs.iter().any(|attr| {
            attr.strip_prefix("domain=")
                .map(|domain| domain.trim_start_matches('.') == "itzon.tv")
                .unwrap_or(false)
        });
        let deleted = attrs.iter().any(|attr| attr == "max-age=0");
        if deleted {
            origin.remove(name.trim());
            if shared {
                delta.shared_deletes.insert(name.trim().to_string());
            }
            continue;
        }
        origin.insert(name.trim().to_string(), value.to_string());
        if shared {
            delta
                .shared_upserts
                .insert(name.trim().to_string(), value.to_string());
        }
    }
    delta
}

pub async fn connect() -> Result<()> {
    if oauth_configured() {
        crate::services::itzon_oauth_service::connect().await
    } else {
        connect_cookie().await
    }
}

pub async fn restore() -> bool {
    crate::services::itzon_oauth_service::restore().await || restore_cookie().await
}

pub async fn disconnect() {
    crate::services::itzon_oauth_service::disconnect();
    disconnect_cookie().await;
}

#[cfg(windows)]
async fn connect_cookie() -> Result<()> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let app = crate::services::providers::app_handle()
        .ok_or_else(|| anyhow!("app handle unavailable for itzon login"))?;
    if let Some(existing) = app.get_webview_window(LOGIN_WINDOW_LABEL) {
        let _ = existing.destroy();
    }
    let login_url = tauri::Url::parse("https://itzon.tv/login?return=https%3A%2F%2Fitzon.tv%2F")?;
    WebviewWindowBuilder::new(&app, LOGIN_WINDOW_LABEL, WebviewUrl::External(login_url))
        .title("Sign in to itzon")
        .inner_size(520.0, 720.0)
        .data_directory(profile_dir())
        .build()
        .map_err(|error| anyhow!("itzon login window failed: {error}"))?;

    let chat_url = resolve_chat_cookie_url().await?;
    let mut connected = None;
    for _ in 0..200 {
        if app.get_webview_window(LOGIN_WINDOW_LABEL).is_none() {
            break;
        }
        if let Ok(origin_cookies) =
            fetch_cookies_from_window(&app, LOGIN_WINDOW_LABEL, ORIGIN).await
        {
            if !origin_cookies.is_empty() {
                if let Ok((info, origin_cookies, chat_delta)) =
                    validate_session(origin_cookies).await
                {
                    let mut chat_cookies =
                        fetch_cookies_from_window(&app, LOGIN_WINDOW_LABEL, chat_url.as_str())
                            .await
                            .unwrap_or_default();
                    for name in chat_delta.shared_deletes {
                        chat_cookies.remove(&name);
                    }
                    chat_cookies.extend(chat_delta.shared_upserts);
                    if !chat_cookies.is_empty() {
                        connected = Some(ItzonSession {
                            origin_cookies,
                            chat_cookies,
                            username: info.username,
                            token: info.token,
                            validated_at: Instant::now(),
                        });
                        break;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    if let Some(window) = app.get_webview_window(LOGIN_WINDOW_LABEL) {
        let _ = window.destroy();
    }
    let connected = connected.ok_or_else(|| anyhow!("itzon sign-in wasn't completed"))?;
    if let Ok(mut session) = session_cell().lock() {
        *session = Some(connected);
        bump_revision();
    }
    Ok(())
}

/// Rehydrate the in-memory native session from the site-owned WebView2 profile.
/// The profile already persists itzon's cookies; this only validates and copies
/// the cookies needed by the native chat socket after an app restart.
#[cfg(windows)]
async fn restore_cookie() -> bool {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let _restore_guard = RESTORE_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    if is_connected() {
        return true;
    }
    let Some(app) = crate::services::providers::app_handle() else {
        return false;
    };
    if let Some(existing) = app.get_webview_window(RESTORE_WINDOW_LABEL) {
        let _ = existing.destroy();
    }

    let origin = match tauri::Url::parse(ORIGIN) {
        Ok(origin) => origin,
        Err(_) => return false,
    };
    if WebviewWindowBuilder::new(&app, RESTORE_WINDOW_LABEL, WebviewUrl::External(origin))
        .title("Restoring itzon session")
        .visible(false)
        .data_directory(profile_dir())
        .build()
        .is_err()
    {
        return false;
    }

    let chat_url = match resolve_chat_cookie_url().await {
        Ok(url) => url,
        Err(error) => {
            log::debug!("[itzon] session restore could not resolve chat edge: {error}");
            if let Some(window) = app.get_webview_window(RESTORE_WINDOW_LABEL) {
                let _ = window.destroy();
            }
            return false;
        }
    };

    let mut restored = None;
    for _ in 0..24 {
        if let Ok(origin_cookies) =
            fetch_cookies_from_window(&app, RESTORE_WINDOW_LABEL, ORIGIN).await
        {
            if !origin_cookies.is_empty() {
                if let Ok((info, origin_cookies, chat_delta)) =
                    validate_session(origin_cookies).await
                {
                    let mut chat_cookies =
                        fetch_cookies_from_window(&app, RESTORE_WINDOW_LABEL, chat_url.as_str())
                            .await
                            .unwrap_or_default();
                    for name in chat_delta.shared_deletes {
                        chat_cookies.remove(&name);
                    }
                    chat_cookies.extend(chat_delta.shared_upserts);
                    if !chat_cookies.is_empty() {
                        restored = Some(ItzonSession {
                            origin_cookies,
                            chat_cookies,
                            username: info.username,
                            token: info.token,
                            validated_at: Instant::now(),
                        });
                        break;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    if let Some(window) = app.get_webview_window(RESTORE_WINDOW_LABEL) {
        let _ = window.destroy();
    }
    if let Some(restored) = restored {
        if let Ok(mut session) = session_cell().lock() {
            *session = Some(restored);
            bump_revision();
            log::info!("[itzon] restored signed-in session from the site profile");
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
async fn restore_cookie() -> bool {
    session_cell()
        .lock()
        .map(|session| session.is_some())
        .unwrap_or(false)
}

#[cfg(not(windows))]
async fn connect_cookie() -> Result<()> {
    Err(anyhow!(
        "itzon web-session login is currently implemented on Windows"
    ))
}

async fn resolve_chat_cookie_url() -> Result<url::Url> {
    let config = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()?
        .get(WS_CONFIG_URL)
        .send()
        .await?
        .error_for_status()?
        .json::<WsConfig>()
        .await?;
    let mut url = url::Url::parse(&config.chat_wss)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("itzon chat URL has no host"))?;
    if host != "itzon.tv" && !host.ends_with(".itzon.tv") {
        return Err(anyhow!("untrusted itzon chat host"));
    }
    url.set_scheme("https")
        .map_err(|_| anyhow!("invalid itzon chat scheme"))?;
    url.set_path("/ws/irc");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

async fn disconnect_cookie() {
    let session = session_cell().lock().ok().and_then(|mut session| {
        let removed = session.take();
        if removed.is_some() {
            bump_revision();
        }
        removed
    });
    if let Some(session) = session {
        if !session.token.is_empty() {
            if let Ok(client) = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build() {
                let _ = client
                    .post("https://itzon.tv/api/main/v1/auth/logout")
                    .bearer_auth(session.token)
                    .header(
                        reqwest::header::COOKIE,
                        cookie_header(&session.origin_cookies),
                    )
                    .send()
                    .await;
            }
        }
    }
    if let Some(app) = crate::services::providers::app_handle() {
        use tauri::Manager;
        if let Some(window) = app.get_webview_window(LOGIN_WINDOW_LABEL) {
            let _ = window.destroy();
        }
    }
    let _ = std::fs::remove_dir_all(profile_dir());
}

#[cfg(windows)]
async fn fetch_cookies_from_window(
    app: &tauri::AppHandle,
    window_label: &str,
    uri: &str,
) -> Result<HashMap<String, String>> {
    use std::sync::Arc;
    use tauri::Manager;
    use tokio::sync::oneshot;

    let webview = app
        .get_webview_window(window_label)
        .ok_or_else(|| anyhow!("itzon login window unavailable"))?;
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
    let tx_for_webview = tx.clone();
    let uri = uri.to_string();
    webview
        .with_webview(move |platform_webview| {
            let result = unsafe { request_cookies(platform_webview, tx_for_webview.clone(), uri) };
            if let Err(error) = result {
                if let Some(sender) = tx_for_webview.lock().unwrap().take() {
                    let _ = sender.send(Err(anyhow!("WebView2 cookie request failed: {error}")));
                }
            }
        })
        .map_err(|error| anyhow!("with_webview failed: {error}"))?;
    rx.await
        .map_err(|_| anyhow!("WebView2 cookie callback dropped"))?
}

#[cfg(windows)]
unsafe fn request_cookies(
    platform_webview: tauri::webview::PlatformWebview,
    tx: std::sync::Arc<
        std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<HashMap<String, String>>>>>,
    >,
    uri: String,
) -> windows::core::Result<()> {
    use webview2_com::GetCookiesCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_2;
    use windows::core::{Interface, HSTRING};

    let core = platform_webview.controller().CoreWebView2()?;
    let manager = core.cast::<ICoreWebView2_2>()?.CookieManager()?;
    let handler = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
        let cookies = extract_cookies(result, list);
        if let Some(sender) = tx.lock().unwrap().take() {
            let _ = sender.send(cookies);
        }
        Ok(())
    }));
    manager.GetCookies(&HSTRING::from(uri), &handler)?;
    Ok(())
}

#[cfg(windows)]
fn extract_cookies(
    completion: windows::core::Result<()>,
    cookie_list: Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2CookieList>,
) -> Result<HashMap<String, String>> {
    use webview2_com::take_pwstr;
    use windows::core::PWSTR;

    completion.map_err(|error| anyhow!("GetCookies failed: {error}"))?;
    let list = cookie_list.ok_or_else(|| anyhow!("WebView2 returned no cookie list"))?;
    let mut count = 0;
    unsafe { list.Count(&mut count) }?;
    let mut cookies = HashMap::new();
    for index in 0..count {
        let cookie = unsafe { list.GetValueAtIndex(index) }?;
        let mut name = PWSTR::null();
        let mut value = PWSTR::null();
        unsafe {
            cookie.Name(&mut name)?;
            cookie.Value(&mut value)?;
        }
        let name = take_pwstr(name);
        let value = take_pwstr(value);
        if !name.is_empty() && !value.is_empty() {
            cookies.insert(name, value);
        }
    }
    Ok(cookies)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session(username: &str) -> ItzonSession {
        ItzonSession {
            origin_cookies: HashMap::from([("itzon_session".to_string(), "test".to_string())]),
            chat_cookies: HashMap::from([("itzon_session".to_string(), "test".to_string())]),
            username: username.to_string(),
            token: "test".to_string(),
            validated_at: Instant::now(),
        }
    }

    #[test]
    fn cookie_header_is_stable_and_does_not_log_attributes() {
        let cookies = HashMap::from([
            ("z".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
        ]);
        assert_eq!(cookie_header(&cookies), "a=1; z=2");
    }

    #[test]
    fn renewed_domain_cookie_is_shared_with_chat_edge() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::SET_COOKIE,
            reqwest::header::HeaderValue::from_static(
                "itzon_session=rotated; Domain=.itzon.tv; Path=/; Secure; HttpOnly",
            ),
        );
        headers.append(
            reqwest::header::SET_COOKIE,
            reqwest::header::HeaderValue::from_static("origin_only=value; Path=/; Secure"),
        );
        let mut origin = HashMap::new();
        let chat = merge_response_cookies(&headers, &mut origin);
        assert_eq!(
            origin.get("itzon_session").map(String::as_str),
            Some("rotated")
        );
        assert_eq!(origin.get("origin_only").map(String::as_str), Some("value"));
        assert_eq!(
            chat.shared_upserts.get("itzon_session").map(String::as_str),
            Some("rotated")
        );
        assert!(!chat.shared_upserts.contains_key("origin_only"));
    }

    #[test]
    fn deleted_domain_cookie_is_removed_from_origin_and_chat_edge() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::SET_COOKIE,
            reqwest::header::HeaderValue::from_static(
                "itzon_session=; Domain=.itzon.tv; Path=/; Max-Age=0; Secure; HttpOnly",
            ),
        );
        let mut origin = HashMap::from([("itzon_session".to_string(), "old".to_string())]);
        let delta = merge_response_cookies(&headers, &mut origin);
        assert!(!origin.contains_key("itzon_session"));
        assert!(delta.shared_deletes.contains("itzon_session"));
        assert!(!delta.shared_upserts.contains_key("itzon_session"));
    }

    #[test]
    fn following_usernames_are_normalized_and_deduplicated() {
        assert_eq!(
            normalize_following(vec![
                " Arcade ".to_string(),
                "arcade".to_string(),
                "Grandpa-Chang".to_string(),
                "bad/name".to_string(),
                "".to_string(),
            ]),
            vec!["arcade".to_string(), "grandpa-chang".to_string()]
        );
    }

    #[test]
    fn expiry_clears_only_the_session_revision_that_was_validated() {
        {
            let mut session = session_cell().lock().expect("session lock");
            *session = Some(test_session("first"));
            bump_revision();
        }
        let expired_revision = revision();
        assert!(clear_memory_session_if_revision(expired_revision));
        assert!(!is_connected());

        {
            let mut session = session_cell().lock().expect("session lock");
            *session = Some(test_session("replacement"));
            bump_revision();
        }
        assert!(!clear_memory_session_if_revision(expired_revision));
        assert_eq!(account_name().as_deref(), Some("replacement"));

        let current_revision = revision();
        assert!(clear_memory_session_if_revision(current_revision));
        assert!(!is_connected());
    }
}
