//! this binary is the main entry point for the lexi telegram bot.
//!
//! it is responsible for:
//! - initializing the application environment (configuration, logging, database connections, http client).
//! - performing initial setup, such as fetching and storing the bot's own user details.
//! - launching the main bot event loop (`lexi::bot::r#loop::run_bot_loop`), which listens for
//!   and processes incoming telegram updates.
//!
//! the core bot logic, including message handling and ai interaction, is delegated to
//! modules within the `lexi::bot` and `lexi::message_processor` crates/modules.
//! this file lives at `src/bin/run_tg_bot.rs`.

use anyhow::{Context, Result};
use reqwest::Client as ReqwestClient;
use std::env;
use std::time::Duration;
use tracing::info;

use lexi::bot::r#loop::{run_bot_loop, TELEGRAM_API_URL};
use lexi::db::{self, Db};
use lexi::env::ENV_CONFIG;
use lexi::log;
use lexi::telegram;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    log::init();

    info!("lexi telegram bot - custom implementation (rust)");

    let db_url = ENV_CONFIG
        .database_url
        .as_ref()
        .expect("DATABASE_URL is required for the main application");
    let db_conn = db::PostgresDb::new(db_url).await?;

    let bot_token = env::var("TELEGRAM_BOT_TOKEN")
        .context("TELEGRAM_BOT_TOKEN not set in environment variables")?;

    let reqwest_client = ReqwestClient::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("failed to build http client")?;

    let openai_api_key = env::var("OPENAI_API_KEY")
        .context("OPENAI_API_KEY not set, it is required for the /v1/responses API.")?;

    info!("fetching bot's own user details...");
    let bot_user_from_api = telegram::get_me(&reqwest_client, TELEGRAM_API_URL, &bot_token)
        .await
        .context("failed to get bot's own user details via getMe API")?;
    info!(?bot_user_from_api, "successfully fetched bot details");

    let bot_db_id = db_conn
        .upsert_user(&bot_user_from_api)
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

    run_bot_loop(db_conn, bot_token, openai_api_key, bot_db_id).await
}
