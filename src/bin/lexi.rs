use async_openai::{config::OpenAIConfig, Client as OpenAIClient};
use eyre::{Context, Result};
use reqwest::Client as ReqwestClient;
use std::env;
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

use lexi::db;
use lexi::run_bot_loop;
use lexi::telegram;
use lexi::TELEGRAM_API_URL;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

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
    let openai_client = OpenAIClient::<OpenAIConfig>::new(); // Specify generic, no wrap_err_with here

    // Get bot's own details and store it
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

    // Run the main bot loop
    // If run_bot_loop returns an Err, it will propagate out of main, terminating the program.
    run_bot_loop(pool, bot_token, reqwest_client, openai_client, bot_db_id).await
}
