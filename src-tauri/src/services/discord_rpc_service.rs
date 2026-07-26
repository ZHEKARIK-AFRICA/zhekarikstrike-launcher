use std::sync::Arc;

use chrono::Utc;
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::constants::{
    DISCORD_BUTTON_LABEL, DISCORD_BUTTON_URL, DISCORD_CLIENT_ID, DISCORD_DETAILS,
    DISCORD_LARGE_IMAGE_KEY, DISCORD_LARGE_IMAGE_TEXT,
};
use crate::error::AppError;
use crate::state::{AppState, DiscordRpcState};

pub async fn start_rich_presence(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    start_rich_presence_with_state(app, state.discord_rpc_state.clone()).await
}

pub async fn stop_rich_presence(app: AppHandle, state: &AppState) -> Result<(), AppError> {
    stop_rich_presence_with_state(app, state.discord_rpc_state.clone()).await
}

pub async fn stop_rich_presence_with_state(
    app: AppHandle,
    rpc_state: Arc<Mutex<DiscordRpcState>>,
) -> Result<(), AppError> {
    let mut state = rpc_state.lock().await;
    let Some(mut client) = state.client.take() else {
        return Ok(());
    };

    let clear_result = client.clear_activity();
    let close_result = client.close();
    state.started_at = None;

    if let Err(error) = clear_result {
        eprintln!("Discord RPC clear failed: {error}");
    }
    if let Err(error) = close_result {
        eprintln!("Discord RPC close failed: {error}");
    }

    app.emit("rich-presence-stopped", ())?;
    Ok(())
}

async fn start_rich_presence_with_state(
    app: AppHandle,
    rpc_state: Arc<Mutex<DiscordRpcState>>,
) -> Result<(), AppError> {
    let mut state = rpc_state.lock().await;
    if state.client.is_some() {
        return Ok(());
    }

    let started_at = Utc::now().timestamp();
    let mut client = DiscordIpcClient::new(DISCORD_CLIENT_ID)
        .map_err(|error| AppError::Unknown(format!("Discord RPC init failed: {error}")))?;

    client
        .connect()
        .map_err(|error| AppError::Unknown(format!("Discord RPC connect failed: {error}")))?;

    let payload = activity::Activity::new()
        .details(DISCORD_DETAILS)
        .timestamps(activity::Timestamps::new().start(started_at))
        .assets(
            activity::Assets::new()
                .large_image(DISCORD_LARGE_IMAGE_KEY)
                .large_text(DISCORD_LARGE_IMAGE_TEXT),
        )
        .buttons(vec![activity::Button::new(
            DISCORD_BUTTON_LABEL,
            DISCORD_BUTTON_URL,
        )]);

    client
        .set_activity(payload)
        .map_err(|error| AppError::Unknown(format!("Discord RPC activity failed: {error}")))?;

    state.started_at = Some(started_at);
    state.client = Some(client);
    app.emit("rich-presence-started", ())?;
    Ok(())
}
