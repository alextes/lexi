//! this module contains the main polling loop for the telegram bot.
//! it is responsible for continuously polling the telegram api for new updates (messages),
//! processing these updates by invoking the `bot::handle_telegram_update` function, and handling
//! potential errors during update fetching or processing. it manages the bot's lifecycle
//! and telegram api interactions at a high level, including bot token and api url configurations.
//! this file lives at `src/bot/loop.rs`.

use crate::db::Db;
use anyhow::{Context, Result};
use std::time::Duration;
use tracing::{debug, error, info, trace, warn};

use super::{handle_telegram_update, BotContext}; // Use super to refer to items in src/bot/mod.rs
use crate::telegram::types::{ApiResponse, Update};

pub const TELEGRAM_API_URL: &str = "https://api.telegram.org/bot";

pub async fn run_bot_loop<D: Db + Clone + Send + Sync + 'static>(
    bot_ctx: BotContext<D>,
) -> Result<()> {
    let bot_token = bot_ctx.bot_token.clone();
    let bot_db_id = bot_ctx.bot_db_id;

    let mut last_update_id = 0;
    info!(bot_db_id, "bot loop started. listening for updates...");

    loop {
        let get_updates_url = format!(
            "{}{}/getUpdates?offset={}&timeout=50",
            TELEGRAM_API_URL,
            &bot_token,
            last_update_id + 1
        );

        debug!("requesting updates: {}", get_updates_url);

        let response = bot_ctx
            .http_client
            .get(&get_updates_url)
            .send()
            .await
            .with_context(|| {
                format!("failed to send getUpdates request to URL: {get_updates_url}")
            })?;

        let status = response.status();
        if status.is_success() {
            let api_response = response
                .json::<ApiResponse<Vec<Update>>>()
                .await
                .context("failed to parse json response from getUpdates")?;

            if api_response.ok {
                if let Some(updates) = api_response.result {
                    if updates.is_empty() {
                        trace!("no new updates received.");
                    } else {
                        info!("received {} updates.", updates.len());
                    }
                    for update in updates {
                        last_update_id = update.update_id;
                        debug!(raw_update_object = ?update, "main loop: received update object from api");

                        if let Err(e) = handle_telegram_update(&bot_ctx, &update).await {
                            error!(
                                update_id = update.update_id,
                                error = %e,
                                "failed to process update via bot::handle_telegram_update. continuing to next update."
                            );
                        }
                    }
                } else {
                    trace!("api response ok, but no updates array in result.");
                }
            } else {
                warn!(
                    description = api_response
                        .description
                        .as_deref()
                        .unwrap_or("unknown error"),
                    error_code = api_response.error_code,
                    "telegram api error (ok=false)"
                );
                if let Some(code) = api_response.error_code {
                    if code == 401 || code == 404 {
                        error!("critical telegram api error ({}). please check your token. exiting loop.", code);
                        return Err(anyhow::anyhow!(
                            "telegram API error {}: {}",
                            code,
                            api_response.description.unwrap_or_default()
                        ));
                    }
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        } else {
            let error_body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %error_body, "http error during getUpdates");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}
