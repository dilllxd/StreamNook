//! OAuth 2.0 Authorization Code + PKCE for an itzon public desktop client.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;

const CLIENT_ID: Option<&str> = option_env!("ITZON_OAUTH_CLIENT_ID");
const AUTHORIZE_URL: &str = "https://itzon.tv/oauth/authorize";
const TOKEN_URL: &str = "https://itzon.tv/api/oauth/token";
const USERINFO_URL: &str = "https://itzon.tv/api/oauth/userinfo";
const CALLBACK_PATH: &str = "/oauth/itzon/callback";
const SCOPES: &str = "identity chat api:read";
const KEYRING_SERVICE: &str = "streamnook_itzon_oauth";
const KEYRING_USER: &str = "default";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OAuthToken {
    access_token: String,
    refresh_token: String,
    client_id: String,
    scope: String,
    username: String,
    expires_at: u64,
    refresh_expires_at: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default = "default_access_lifetime")]
    expires_in: u64,
    #[serde(default = "default_refresh_lifetime")]
    refresh_expires_in: u64,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct UserInfo {
    username: String,
}

#[derive(Deserialize)]
struct OAuthError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

pub struct ChatCredential {
    pub access_token: String,
    pub username: String,
}

struct OAuthSession {
    verifier: String,
    challenge: String,
    state: String,
}

static TOKEN: OnceLock<Mutex<Option<OAuthToken>>> = OnceLock::new();
static REFRESH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn token_cell() -> &'static Mutex<Option<OAuthToken>> {
    TOKEN.get_or_init(|| Mutex::new(load_persisted()))
}

fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    REFRESH_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn default_access_lifetime() -> u64 {
    60 * 60
}

fn default_refresh_lifetime() -> u64 {
    30 * 24 * 60 * 60
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn valid_client_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn configured_client_id() -> Option<&'static str> {
    CLIENT_ID
        .map(str::trim)
        .filter(|value| valid_client_id(value))
}

pub fn is_configured() -> bool {
    configured_client_id().is_some()
}

pub fn has_account() -> bool {
    token_cell()
        .lock()
        .map(|token| token.is_some())
        .unwrap_or(false)
}

pub fn account_name() -> Option<String> {
    token_cell().lock().ok().and_then(|token| {
        token
            .as_ref()
            .map(|token| token.username.clone())
            .filter(|username| !username.is_empty())
    })
}

fn persist(token: &OAuthToken) -> Result<()> {
    let json = serde_json::to_string(token).context("could not serialize itzon OAuth token")?;
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .context("could not open the operating-system credential store")?
        .set_password(&json)
        .context("could not save the itzon OAuth token")
}

fn load_persisted() -> Option<OAuthToken> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()?;
    let json = entry.get_password().ok()?;
    let token = serde_json::from_str::<OAuthToken>(&json).ok()?;
    valid_token_shape(&token).then_some(token)
}

fn clear_persisted() {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        let _ = entry.delete_credential();
    }
}

fn valid_token_shape(token: &OAuthToken) -> bool {
    !token.access_token.is_empty()
        && !token.refresh_token.is_empty()
        && valid_client_id(&token.client_id)
        && valid_username(&token.username)
        && has_required_scopes(&token.scope)
}

fn valid_username(username: &str) -> bool {
    !username.is_empty()
        && username.len() <= 64
        && username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn has_required_scopes(scope: &str) -> bool {
    let scopes = scope
        .split_whitespace()
        .collect::<std::collections::HashSet<_>>();
    ["identity", "chat", "api:read"]
        .into_iter()
        .all(|required| scopes.contains(required))
}

fn base64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_base64_url(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut value);
    base64_url(&value)
}

fn create_session() -> OAuthSession {
    let verifier = random_base64_url(32);
    OAuthSession {
        challenge: base64_url(&Sha256::digest(verifier.as_bytes())),
        verifier,
        state: random_base64_url(32),
    }
}

fn authorization_url(
    client_id: &str,
    redirect_uri: &str,
    session: &OAuthSession,
) -> Result<String> {
    let mut url = url::Url::parse(AUTHORIZE_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("state", &session.state)
        .append_pair("code_challenge", &session.challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.into())
}

pub async fn connect() -> Result<()> {
    let client_id = configured_client_id().ok_or_else(|| {
        anyhow!("itzon OAuth registration is pending; ITZON_OAUTH_CLIENT_ID is not configured")
    })?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("could not start the local itzon OAuth callback")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let session = create_session();
    let authorize_url = authorization_url(client_id, &redirect_uri, &session)?;

    use tauri_plugin_opener::OpenerExt;
    crate::services::providers::app_handle()
        .ok_or_else(|| anyhow!("app handle unavailable for itzon OAuth"))?
        .opener()
        .open_url(&authorize_url, None::<String>)
        .context("could not open the itzon authorization page")?;

    let code = timeout(
        Duration::from_secs(180),
        accept_redirect(listener, &session.state),
    )
    .await
    .map_err(|_| anyhow!("itzon authorization timed out"))??;
    let response = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()?
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code.as_str()),
            ("code_verifier", session.verifier.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .context("itzon token exchange failed")?;
    let token = parse_token_response(response, None).await?;
    let user = fetch_identity(&token.access_token).await?;
    let username = user.username.trim().to_lowercase();
    if !valid_username(&username) {
        return Err(anyhow!("itzon returned an invalid OAuth username"));
    }
    let stored = OAuthToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        client_id: client_id.to_string(),
        scope: token.scope,
        username,
        expires_at: now().saturating_add(token.expires_in.max(1)),
        refresh_expires_at: now().saturating_add(token.refresh_expires_in.max(1)),
    };
    store(stored)?;
    Ok(())
}

pub async fn restore() -> bool {
    if !has_account() {
        return false;
    }
    matches!(chat_credential().await, Ok(Some(_)))
}

pub fn disconnect() {
    clear_persisted();
    let changed = token_cell()
        .lock()
        .map(|mut token| token.take().is_some())
        .unwrap_or(false);
    if changed {
        crate::services::itzon_auth_service::oauth_changed();
    }
}

pub async fn chat_credential() -> Result<Option<ChatCredential>> {
    let token = match access_token().await? {
        Some(token) => token,
        None => return Ok(None),
    };
    Ok(Some(ChatCredential {
        access_token: token.access_token,
        username: token.username,
    }))
}

async fn access_token() -> Result<Option<OAuthToken>> {
    let current = token_cell().lock().ok().and_then(|token| token.clone());
    let Some(current) = current else {
        return Ok(None);
    };
    if current.expires_at > now().saturating_add(60) {
        return Ok(Some(current));
    }

    let _guard = refresh_lock().lock().await;
    let current = token_cell().lock().ok().and_then(|token| token.clone());
    let Some(current) = current else {
        return Ok(None);
    };
    if current.expires_at > now().saturating_add(60) {
        return Ok(Some(current));
    }
    if current.refresh_expires_at <= now() {
        disconnect();
        return Ok(None);
    }

    let response = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()?
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", current.client_id.as_str()),
            ("refresh_token", current.refresh_token.as_str()),
        ])
        .send()
        .await
        .context("itzon OAuth refresh failed")?;
    let refreshed = match parse_token_response(response, Some(&current.scope)).await {
        Ok(token) => token,
        Err(error) => {
            if error.to_string().contains("invalid_grant") {
                disconnect();
            }
            return Err(error);
        }
    };
    let next = OAuthToken {
        access_token: refreshed.access_token,
        refresh_token: refreshed.refresh_token,
        client_id: current.client_id,
        scope: refreshed.scope,
        username: current.username,
        expires_at: now().saturating_add(refreshed.expires_in.max(1)),
        refresh_expires_at: now().saturating_add(refreshed.refresh_expires_in.max(1)),
    };
    store(next.clone())?;
    Ok(Some(next))
}

fn store(token: OAuthToken) -> Result<()> {
    if !valid_token_shape(&token) {
        return Err(anyhow!("itzon returned an incomplete OAuth token"));
    }
    persist(&token)?;
    token_cell()
        .lock()
        .map_err(|_| anyhow!("itzon OAuth token state is unavailable"))?
        .replace(token);
    crate::services::itzon_auth_service::oauth_changed();
    Ok(())
}

async fn parse_token_response(
    response: reqwest::Response,
    previous_scope: Option<&str>,
) -> Result<TokenResponse> {
    let status = response.status();
    if !status.is_success() {
        let error = response.json::<OAuthError>().await.unwrap_or(OAuthError {
            error: String::new(),
            error_description: String::new(),
        });
        let detail = match (error.error.is_empty(), error.error_description.is_empty()) {
            (false, false) => format!("{}: {}", error.error, error.error_description),
            (false, true) => error.error,
            (true, false) => error.error_description,
            (true, true) => "OAuth request rejected".to_string(),
        };
        return Err(anyhow!("itzon OAuth error (HTTP {status}): {detail}"));
    }
    let mut token = response
        .json::<TokenResponse>()
        .await
        .context("invalid itzon OAuth token response")?;
    if token.scope.trim().is_empty() {
        token.scope = previous_scope.unwrap_or_default().to_string();
    }
    if token.access_token.is_empty()
        || token.refresh_token.is_empty()
        || !has_required_scopes(&token.scope)
    {
        return Err(anyhow!("itzon returned an incomplete OAuth token"));
    }
    Ok(token)
}

async fn fetch_identity(access_token: &str) -> Result<UserInfo> {
    let response = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()?
        .get(USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .context("could not fetch the itzon OAuth identity")?
        .error_for_status()
        .context("itzon OAuth identity request was rejected")?;
    response
        .json::<UserInfo>()
        .await
        .context("invalid itzon OAuth identity response")
}

async fn accept_redirect(listener: TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut stream, peer) = listener.accept().await?;
        if !peer.ip().is_loopback() {
            continue;
        }
        let mut request = vec![0_u8; 8192];
        let read = timeout(Duration::from_secs(5), stream.read(&mut request))
            .await
            .map_err(|_| anyhow!("itzon OAuth callback read timed out"))??;
        let first_line = String::from_utf8_lossy(&request[..read])
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        let target = first_line
            .strip_prefix("GET ")
            .and_then(|line| line.split_whitespace().next())
            .unwrap_or_default();
        match callback_code(target, expected_state) {
            Ok(Some(code)) => {
                write_callback_response(
                    &mut stream,
                    200,
                    "Authorization received. Return to StreamNook to finish signing in.",
                )
                .await;
                return Ok(code);
            }
            Ok(None) => {
                write_callback_response(&mut stream, 404, "Not found").await;
            }
            Err(error) => {
                write_callback_response(&mut stream, 400, "Authorization was not completed.").await;
                return Err(error);
            }
        }
    }
}

fn callback_code(target: &str, expected_state: &str) -> Result<Option<String>> {
    let url = url::Url::parse(&format!("http://127.0.0.1{target}"))?;
    if url.path() != CALLBACK_PATH {
        return Ok(None);
    }
    let values = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if values.get("state").map(|value| value.as_ref()) != Some(expected_state) {
        return Err(anyhow!("itzon authorization state mismatch"));
    }
    if let Some(error) = values.get("error") {
        return Err(anyhow!("itzon authorization was not completed: {error}"));
    }
    let code = values
        .get("code")
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("itzon authorization returned no code"))?;
    Ok(Some(code))
}

async fn write_callback_response(stream: &mut tokio::net::TcpStream, status: u16, message: &str) {
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui;background:#111;color:#eee;text-align:center;padding-top:80px\"><h2>{message}</h2><p>You can close this tab.</p></body></html>"
    );
    let reason = if status == 200 { "OK" } else { "Error" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_rfc7636_s256_session() {
        let session = create_session();
        assert_eq!(session.verifier.len(), 43);
        assert_eq!(session.challenge.len(), 43);
        assert_eq!(session.state.len(), 43);
        assert_eq!(
            session.challenge,
            base64_url(&Sha256::digest(session.verifier.as_bytes()))
        );
    }

    #[test]
    fn builds_public_client_authorization_url() {
        let session = create_session();
        let url = authorization_url(
            "0123456789abcdef0123456789abcdef",
            "http://127.0.0.1:49152/oauth/itzon/callback",
            &session,
        )
        .unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        let query = parsed
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("itzon.tv"));
        assert_eq!(query.get("scope").map(|value| value.as_ref()), Some(SCOPES));
        assert_eq!(
            query
                .get("code_challenge_method")
                .map(|value| value.as_ref()),
            Some("S256")
        );
        assert!(!query.contains_key("client_secret"));
    }

    #[test]
    fn callback_requires_matching_state_and_exact_path() {
        assert_eq!(
            callback_code("/oauth/itzon/callback?code=abc&state=expected", "expected").unwrap(),
            Some("abc".to_string())
        );
        assert!(callback_code("/oauth/itzon/callback?code=abc&state=wrong", "expected").is_err());
        assert_eq!(callback_code("/favicon.ico", "expected").unwrap(), None);
    }

    #[test]
    fn validates_client_id_and_required_scopes() {
        assert!(valid_client_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_client_id("not-a-client-id"));
        assert!(has_required_scopes("api:read identity chat"));
        assert!(!has_required_scopes("identity api:read"));
    }
}
