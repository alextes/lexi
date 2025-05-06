use anyhow::{Context, Result}; // Use anyhow::Result
use async_openai::{
    config::OpenAIConfig, // Import OpenAIConfig
    Client as OpenAIClient,
};
use reqwest::Client as ReqwestClient;
use sqlx::SqlitePool; // Keep for type annotation if needed, though pool comes from db module
use std::env;
use std::time::Duration;
use tracing::{debug, error, info, trace, warn}; // info, warn used by handler indirectly via db
use tracing_subscriber::EnvFilter;

mod db;
mod handler;
mod telegram_client;
mod types; // Added telegram_client module

const TELEGRAM_API_URL: &str = "https://api.telegram.org/bot";

async fn run_bot_loop(
    pool: SqlitePool,
    bot_token: String,
    http_client: ReqwestClient,
    openai_client: OpenAIClient<OpenAIConfig>, // Specify generic
    bot_db_id: i64,                            // Added bot's own database ID
) -> Result<()> {
    let mut last_update_id = 0;
    info!(bot_db_id, "bot loop started. listening for updates..."); // Log bot_db_id

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
                .json::<types::ApiResponse<Vec<types::Update>>>()
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

                        // Pass http_client, TELEGRAM_API_URL, and bot_token to handler
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

#[tokio::main]
async fn main() -> Result<()> {
    // Changed to anyhow::Result
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    info!("lexi telegram bot - custom implementation (rust)");

    let database_url =
        env::var("DATABASE_URL").context("DATABASE_URL not set in environment variables")?;

    let pool = db::initialize_database(&database_url).await?;

    let bot_token = env::var("TELEGRAM_BOT_TOKEN")
        .context("TELEGRAM_BOT_TOKEN not set in environment variables")?;

    // Initialize Reqwest Client
    let reqwest_client = ReqwestClient::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("failed to build http client")?;

    // Check for OPENAI_API_KEY before creating client
    env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set in environment variables")?;
    let openai_client = OpenAIClient::<OpenAIConfig>::new(); // Specify generic, no with_context here

    // Get bot's own details and store it
    info!("fetching bot's own user details...");
    let bot_user_from_api = telegram_client::get_me(&reqwest_client, TELEGRAM_API_URL, &bot_token)
        .await
        .context("failed to get bot's own user details via getMe API")?;
    info!(?bot_user_from_api, "successfully fetched bot details");

    let bot_db_id = db::upsert_user(&pool, &bot_user_from_api)
        .await
        .with_context(|| {
            format!(
                "failed to upsert bot's own user data (telegram_id: {}) into database",
                bot_user_from_api.id
            )
        })?;
    info!(
        bot_telegram_id = bot_user_from_api.id,
        bot_db_id, "bot user data upserted into database"
    );

    // Run the main bot loop
    // If run_bot_loop returns an Err, it will propagate out of main, terminating the program.
    run_bot_loop(pool, bot_token, reqwest_client, openai_client, bot_db_id).await
}
