//! this cli application provides a way to test the core ai message processing logic
//! (`message_processor::process_single_prompt_for_cli` which delegates to `openai_chat`)
//! directly from the command line, bypassing the full telegram bot loop and database interactions
//! for conversation history. it takes a text prompt as input and outputs the ai's final response.
//! this is useful for quick iterative testing of the ai's capabilities, tool usage, and prompt handling
//! without needing to interact with a live telegram bot.

use clap::Parser;
use eyre::{Context, Result};
use lexi::env::ENV_CONFIG;
use lexi::log;
use lexi::message_processor::{self, HandlerContext as MessageProcessorHandlerContext}; // Aliasing for clarity
use reqwest::Client as ReqwestClient;
use std::time::Duration;
use tracing::{error, info};

// Default IDs for testing if not provided or if we simplify context creation
const DEFAULT_TEST_CHAT_ID: i64 = 12345; // Telegram chat ID for logging/context in message_processor
const DEFAULT_BOT_DB_ID: i32 = -1; // Placeholder for HandlerContext

/// a cli to test the `message_processor` and ai tool usage.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct CliArgs {
    /// the prompt to send to the ai assistant
    prompt: String,

    #[clap(long, default_value_t = DEFAULT_TEST_CHAT_ID)]
    logging_chat_id: i64, // Renamed to reflect its purpose more accurately for message_processor
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    dotenv::dotenv().ok();

    log::init();

    info!("message_processor_cli starting...");

    let cli = CliArgs::parse();

    let db_url = ENV_CONFIG
        .database_url
        .as_ref()
        .expect("DATABASE_URL is required.");
    let db = lexi::db::PostgresDb::new(db_url).await?;

    let http_client = ReqwestClient::builder()
        .timeout(Duration::from_secs(120)) // Increased timeout for potentially longer AI calls
        .build()
        .context("failed to build http client")?;

    let openai_api_key =
        std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set, it is required.")?;

    // Create the simplified HandlerContext for message_processor
    let mp_handler_ctx = MessageProcessorHandlerContext {
        db,
        http_client: &http_client,
        bot_db_id: DEFAULT_BOT_DB_ID, // A dummy value, as it's less relevant for pure CLI testing of AI
        openai_api_key: &openai_api_key,
    };

    info!(
        "Calling message_processor::process_single_prompt_for_cli with prompt: '{}', logging_chat_id: {}",
        cli.prompt,
        cli.logging_chat_id
    );

    match message_processor::process_single_prompt_for_cli(
        &mp_handler_ctx,
        &cli.prompt,
        cli.logging_chat_id,
    )
    .await
    {
        Ok((final_text, response_id)) => {
            info!(
                "AI processing complete. Response ID: {}. Final text:",
                response_id
            );
            println!("ai interaction successful.");
            println!("response id: {response_id}");
            println!("final text:\n{final_text}");
        }
        Err(e) => {
            error!(error = %e, "error during ai processing with message_processor::process_single_prompt_for_cli");
            eprintln!("error during ai processing: {e:?}");
        }
    }

    Ok(())
}
