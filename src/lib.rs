pub mod db;
pub mod env;
pub mod handler;
pub mod log;
pub mod telegram;
pub mod tools;

use anyhow::{Context, Result}; // Use anyhow::Result
use async_openai::{
    config::OpenAIConfig, // Import OpenAIConfig
    Client as OpenAIClient,
};
use reqwest::Client as ReqwestClient;
use sqlx::PgPool;
use std::time::Duration;
use tracing::{debug, error, info, trace, warn};

pub const TELEGRAM_API_URL: &str = "https://api.telegram.org/bot";

pub async fn run_bot_loop(
    pool: PgPool,
    bot_token: String,
    http_client: ReqwestClient,
    openai_client: OpenAIClient<OpenAIConfig>,
    bot_db_id: i32,
) -> Result<()> {
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

        let response = http_client
            .get(&get_updates_url)
            .send()
            .await
            .with_context(|| {
                format!(
                    "failed to send getUpdates request to URL: {}",
                    get_updates_url
                )
            })?;

        let status = response.status();
        if status.is_success() {
            let api_response = response
                .json::<telegram::types::ApiResponse<Vec<telegram::types::Update>>>()
                .await
                .with_context(|| "failed to parse json response from getUpdates")?;

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

                        handler::process_update(
                            &pool,
                            &update,
                            &openai_client,
                            &http_client,
                            TELEGRAM_API_URL,
                            &bot_token,
                            bot_db_id, // Pass bot_db_id
                        )
                        .await
                        .with_context(|| {
                            format!("error processing update_id: {}", update.update_id)
                        })?;
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
                        // This is a critical error, so we return an error to stop the bot loop.
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
            // Log HTTP errors but continue loop, as they might be transient.
            // For critical HTTP errors, we might want to return Err here too.
            error!(status = %status, body = %error_body, "http error during getUpdates");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    } // Loop continues
      // Unreachable in normal operation unless loop breaks, which it currently doesn't explicitly.
      // If the loop were to terminate gracefully, Ok(()) would be returned here.
}
