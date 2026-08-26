//! Tauri commands for the multi-account registry. Phase 1 exposes only the
//! read side (list + count) that later phases and the "send as" picker build on.
//! Add / remove / set-primary commands arrive with the add-account flow.

use crate::models::settings::AppState;
use crate::services::account_store::{AccountStore, StoredAccount};
use crate::services::twitch_service::{TwitchService, SCOPES};
use tauri::{AppHandle, State};

/// Every linked account, primary first. The frontend uses this to render the
/// account list and the "send as" picker.
#[tauri::command]
pub async fn list_twitch_accounts() -> Result<Vec<StoredAccount>, String> {
    Ok(AccountStore::list())
}

/// Number of linked accounts. The "send as" picker is only shown when this is
/// 2 or more, so a single-account user sees no change.
#[tauri::command]
pub async fn get_twitch_account_count() -> Result<usize, String> {
    Ok(AccountStore::count())
}

/// Link a new secondary account with Twitch's public-client Device Code Flow.
/// The token is filed directly as a secondary and never occupies the primary
/// account's storage slot.
#[tauri::command]
pub async fn add_twitch_account(app: AppHandle) -> Result<StoredAccount, String> {
    use tauri_plugin_opener::OpenerExt;

    let flow = TwitchService::start_device_authorization(SCOPES)
        .await
        .map_err(|e| e.to_string())?;
    app.opener()
        .open_url(flow.verification_uri, None::<String>)
        .map_err(|e| format!("Failed to open browser: {}", e))?;
    let token = TwitchService::complete_device_authorization(
        &flow.device_code,
        flow.interval,
        flow.expires_in,
        SCOPES,
    )
    .await
    .map_err(|e| e.to_string())?;

    AccountStore::add_secondary(token)
        .await
        .map_err(|e| e.to_string())
}

/// Unlink a secondary account (removes its stored token and registry entry).
/// The primary cannot be removed here.
#[tauri::command]
pub async fn remove_twitch_account(user_id: String) -> Result<(), String> {
    AccountStore::remove_secondary(&user_id).map_err(|e| e.to_string())
}

/// Promote a linked account to the main account you watch and stream as. Moves
/// its token into the primary slot and demotes the previous main to a linked
/// account (kept, never deleted). Returns the newly-active account. The frontend
/// re-establishes the watched identity (and reconnects chat) afterward.
#[tauri::command]
pub async fn set_active_twitch_account(
    user_id: String,
    state: State<'_, AppState>,
) -> Result<StoredAccount, String> {
    let result = AccountStore::set_active(&user_id)
        .await
        .map_err(|e| e.to_string());
    if result.is_ok() {
        // The active account changed: re-harvest the web session for the new
        // account on the next resolve, and drop any Turbo/sub verdict cached
        // against the previous account's token.
        state.twitch_auth.on_account_changed().await;
        crate::services::auth_proxy::clear_entitlement_caches();
        // The drops/points credential belongs to the previous account; clear it
        // so the heartbeat doesn't keep crediting the account you switched away
        // from. The new account re-authorizes drops separately when wanted.
        let _ = crate::services::drops_auth_service::DropsAuthService::logout().await;
    }
    result
}

/// Sign out of the current main. If other accounts are linked, the most recently
/// added one is promoted to main and returned; otherwise this is a full sign-out
/// and returns `null`. The frontend re-establishes identity when an account is
/// returned, or drops to the logged-out state when it is null.
#[tauri::command]
pub async fn sign_out_active_twitch_account(
    state: State<'_, AppState>,
) -> Result<Option<StoredAccount>, String> {
    let result = AccountStore::sign_out_active()
        .await
        .map_err(|e| e.to_string());
    match &result {
        // Another linked account was promoted into the main slot.
        Ok(Some(_)) => state.twitch_auth.on_account_changed().await,
        // Full sign-out: reads stay logged-out until the next login.
        Ok(None) => state.twitch_auth.on_logged_out().await,
        Err(_) => {}
    }
    if result.is_ok() {
        crate::services::auth_proxy::clear_entitlement_caches();
        // The outgoing account's drops/points credential is now wrong whether we
        // promoted another account or fully signed out; clear it.
        let _ = crate::services::drops_auth_service::DropsAuthService::logout().await;
    }
    result
}
