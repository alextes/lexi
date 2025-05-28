// src/bin/message_processor_cli.rs

use clap::Parser;
use eyre::{Context, Result};
use lexi::env::ENV_CONFIG;
use lexi::log;
use lexi::message_processor::{process_single_prompt_for_cli, HandlerContext};
use reqwest::Client as ReqwestClient;
use std::time::Duration;
use tracing::{error, info};

// Default IDs for testing if not provided or if we simplify context creation
const DEFAULT_TEST_CHAT_ID: i64 = 12345; // Telegram chat ID for logging/context
                                         // const DEFAULT_TEST_LOCAL_CHAT_ID: i32 = 1; // No longer used by this CLI
                                         // const DEFAULT_TEST_USER_ID: i64 = 54321; // No longer used by this CLI
const DEFAULT_BOT_DB_ID: i32 = -1; // Placeholder for HandlerContext

/// a cli to test the message_processor and ai tool usage.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct CliArgs {
    /// the prompt to send to the ai assistant
    prompt: String,

    #[clap(long, default_value_t = DEFAULT_TEST_CHAT_ID)]
    telegram_chat_id: i64,
    // #[clap(long, default_value_t = DEFAULT_TEST_USER_ID)] // No longer used
    // telegram_user_id: i64,

    // #[clap(long, default_value_t = DEFAULT_TEST_LOCAL_CHAT_ID)] // No longer used
    // local_chat_id: i32,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    dotenv::dotenv().ok();

    log::init(ENV_CONFIG.log_json, ENV_CONFIG.log_perf);

    info!("message_processor_cli starting...");

    let cli = CliArgs::parse();

    let db_url = ENV_CONFIG
        .database_url
        .as_ref()
        .expect("DATABASE_URL is required.");
    // Note: For mevdb_query_tool to work, MEVDB_DATABASE_URL must be set in .env
    // For beacon_slot_check_tool, BEACON_URL must be set in .env
    // These are handled by the tools themselves or expect() in their setup.

    // We don't initialize a pool here as generate_and_send_ai_reply expects one in HandlerContext.
    // The actual MEVDB pool is created on-demand by its tool.
    // The main pool is needed for chat history, etc.
    let pool = lexi::db::initialize_database(db_url).await?;

    let http_client = ReqwestClient::builder()
        .timeout(Duration::from_secs(120)) // Increased timeout for potentially longer AI calls
        .build()
        .context("failed to build http client")?;

    let openai_api_key =
        std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set, it is required.")?;

    // Dummy bot token, as we are not calling telegram API directly for sending messages here
    let dummy_bot_token = "dummy_token_for_cli".to_string();
    let dummy_telegram_api_url = "https://api.telegram.org/bot";

    let handler_ctx = HandlerContext {
        pool: &pool,
        http_client: &http_client,
        api_base_url: dummy_telegram_api_url,
        bot_token: &dummy_bot_token,
        bot_db_id: DEFAULT_BOT_DB_ID,
        openai_api_key: &openai_api_key,
    };

    info!(
        "Calling process_single_prompt_for_cli with prompt: '{}'",
        cli.prompt
    );

    match process_single_prompt_for_cli(&handler_ctx, &cli.prompt, cli.telegram_chat_id).await {
        Ok((final_text, response_id)) => {
            info!(
                "AI processing complete. Response ID: {}. Final text:",
                response_id
            );
            println!("ai interaction successful.");
            println!("response id: {}", response_id);
            println!("final text:\n{}", final_text);
        }
        Err(e) => {
            error!(error = %e, "error during ai processing with process_single_prompt_for_cli");
            eprintln!("error during ai processing: {:?}", e);
        }
    }

    Ok(())
}
