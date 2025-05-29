use eyre::{Context, Result};
use reqwest::Client as ReqwestClient;
use std::env;
use std::time::Duration;
use tracing::info;

use lexi::bot::r#loop::{run_bot_loop, TELEGRAM_API_URL};
use lexi::db;
use lexi::env::ENV_CONFIG;
use lexi::log;
use lexi::telegram;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    dotenv::dotenv().ok();

    log::init();

    info!("lexi telegram bot - custom implementation (rust)");

    let db_url = ENV_CONFIG
        .database_url
        .as_ref()
        .expect("DATABASE_URL is required for the main application");
    let pool = db::initialize_database(db_url).await?;

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

    let bot_db_id = db::upsert_user(&pool, &bot_user_from_api)
        .await
        .wrap_err_with(|| {
            format!(
                "failed to upsert bot's own user data (telegram_id: {}) into database",
                bot_user_from_api.id
            )
        })?;
    info!(
        bot_telegram_id = bot_user_from_api.id,
        bot_db_id, "bot user data upserted into database"
    );

    run_bot_loop(pool, bot_token, reqwest_client, openai_api_key, bot_db_id).await
}
