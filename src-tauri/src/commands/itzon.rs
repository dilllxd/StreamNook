use crate::services::itzon_media::{self, ItzonChannel, ItzonExplore};

#[tauri::command]
pub async fn get_itzon_explore() -> Result<ItzonExplore, String> {
    itzon_media::explore()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_itzon_channel(username: String) -> Result<ItzonChannel, String> {
    itzon_media::channel(&username)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_itzon_following() -> Result<Vec<String>, String> {
    crate::services::itzon_auth_service::followed_usernames()
        .await
        .map_err(|error| error.to_string())
}
