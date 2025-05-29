//! this module contains the main polling loop for the telegram bot.
//! it is responsible for continuously polling the telegram api for new updates (messages),
//! processing these updates by invoking the `bot::handle_telegram_update` function, and handling
//! potential errors during update fetching or processing. it manages the bot's lifecycle
//! and telegram api interactions at a high level, including bot token and api url configurations.
//! this file lives at `src/bot/loop.rs`.

use eyre::{Context, Result};
use reqwest::Client as ReqwestClient;
use sqlx::PgPool;
use std::time::Duration;
use tracing::{debug, error, info, trace, warn};

use super::{handle_telegram_update, BotContext}; // Use super to refer to items in src/bot/mod.rs
use crate::telegram::types::{ApiResponse, Update};

pub const TELEGRAM_API_URL: &str = "https://api.telegram.org/bot";

pub async fn run_bot_loop(
    pool: PgPool,
    bot_token: String,
    http_client: ReqwestClient,
    openai_api_key: String,
    bot_db_id: i32,
) -> Result<()> {
    let mut last_update_id = 0;
    info!(
        bot_db_id,
        "(bot::loop) bot loop started. listening for updates..."
    ); // updated log prefix

    let bot_ctx = BotContext {
        // Use BotContext from super
        pool: &pool,
        http_client: &http_client,
        api_base_url: TELEGRAM_API_URL,
        bot_token: &bot_token,
        bot_db_id,
        openai_api_key: &openai_api_key,
    };

    loop {
        let get_updates_url = format!(
            "{}{}/getUpdates?offset={}&timeout=50",
            TELEGRAM_API_URL,
            &bot_token,
            last_update_id + 1
        );

        debug!("(bot::loop) requesting updates: {}", get_updates_url);

        let response = http_client
            .get(&get_updates_url)
            .send()
            .await
            .wrap_err_with(|| {
                format!("(bot::loop) failed to send getUpdates request to URL: {get_updates_url}")
            })?;

        let status = response.status();
        if status.is_success() {
            let api_response = response
                .json::<ApiResponse<Vec<Update>>>()
                .await
                .wrap_err_with(|| "(bot::loop) failed to parse json response from getUpdates")?;

            if api_response.ok {
                if let Some(updates) = api_response.result {
                    if updates.is_empty() {
                        trace!("(bot::loop) no new updates received.");
                    } else {
                        info!("(bot::loop) received {} updates.", updates.len());
                    }
                    for update in updates {
                        last_update_id = update.update_id;
                        debug!(raw_update_object = ?update, "(bot::loop) main loop: received update object from api");

                        if let Err(e) = handle_telegram_update(&bot_ctx, &update).await
                        // Call handle_telegram_update from super
                        {
                            error!(
                                update_id = update.update_id,
                                error = %e,
                                "(bot::loop) failed to process update via bot::handle_telegram_update. continuing to next update."
                            );
                        }
                    }
                } else {
                    trace!("(bot::loop) api response ok, but no updates array in result.");
                }
            } else {
                warn!(
                    description = api_response
                        .description
                        .as_deref()
                        .unwrap_or("unknown error"),
                    error_code = api_response.error_code,
                    "(bot::loop) telegram api error (ok=false)"
                );
                if let Some(code) = api_response.error_code {
                    if code == 401 || code == 404 {
                        error!("(bot::loop) critical telegram api error ({}). please check your token. exiting loop.", code);
                        return Err(eyre::eyre!(
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
            error!(status = %status, body = %error_body, "(bot::loop) http error during getUpdates");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}
