// This file is being renamed from test_tool.rs
// The existing content will be preserved during the rename operation,
// and then modified in the subsequent step.

use eyre::{Context, Result};
use async_openai::{config::OpenAIConfig, Client as OpenAIClient};
use clap::Parser;
use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::env;
use tracing::{error, info};

// use the tools from the library
use lexi::tools::sql_select;

/// a cli to test ai tool usage.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// instruction to give to the ai
    #[clap(short, long)]
    instruction: String,

    /// openai model to use for the test
    #[clap(long, default_value = "gpt-4-turbo")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenv().ok(); // load .env file if present

    let args = Args::parse();
    info!(instruction = %args.instruction, model = %args.model, "starting ai tool cli");

    let database_url = env::var("DATABASE_URL")
        .context("test_database_url not set. check .env or environment for the cli.")?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .context("failed to connect to the test postgresql database (cli)")?;
    info!(url = %database_url, "cli connected to test database specified by test_database_url.");

    // --- end database setup ---

    // --- openai client setup ---
    let _openai_api_key =
        env::var("OPENAI_API_KEY").context("openai_api_key not set. check .env or environment.")?;
    let openai_client = OpenAIClient::<OpenAIConfig>::new(); // uses openai_api_key from env
    info!("openai client initialized.");
    // --- end openai client setup ---

    // --- execute tool processing ---
    info!("cli processing instruction using the sql_select tool...");
    match sql_select::process_instruction_with_sql_tool(
        &openai_client,
        &pool, // Pass the pool connected to test_database_url
        args.instruction,
        &args.model,
    )
    .await
    {
        Ok(final_response) => {
            info!("cli successfully processed instruction. final ai response:");
            println!("\n🤖 lexi's final response:\n--------------------------\n{}\n--------------------------", final_response);
        }
        Err(e) => {
            error!(error = %e, "cli error processing instruction with sql_select tool");
            eprintln!("\n❌ error during tool processing (cli): {:#}", e);
        }
    }
    // --- end execute tool processing ---

    pool.close().await;
    info!("cli test database pool closed.");

    Ok(())
}
