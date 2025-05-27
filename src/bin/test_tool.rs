use anyhow::{Context, Result};
use async_openai::{config::OpenAIConfig, Client as OpenAIClient};
use clap::Parser;
use dotenv::dotenv;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::env;
use tracing::{error, info};

// Use the tools from the library
use lexi::tools::sql_query_tool;

/// A CLI to test AI tool usage.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Instruction to give to the AI
    #[clap(short, long)]
    instruction: String,

    /// OpenAI model to use for the test
    #[clap(long, default_value = "gpt-4-turbo")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenv().ok();

    let args = Args::parse();
    info!(instruction = %args.instruction, model = %args.model, "starting ai tool test");

    // --- Database Setup ---
    let database_url = env::var("TEST_DATABASE_URL")
        .context("test_database_url not set. check .env or environment.")?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .context("failed to connect to the test postgresql database")?;
    info!(url = %database_url, "connected to test database.");

    // Prepare the schema specifically for the SQL tool
    sql_query_tool::prepare_sql_tool_schema(&pool)
        .await
        .context("failed to prepare sql tool schema")?;
    // --- End Database Setup ---

    // --- OpenAI Client Setup ---
    let _openai_api_key =
        env::var("OPENAI_API_KEY").context("openai_api_key not set. check .env or environment.")?;
    let openai_client = OpenAIClient::<OpenAIConfig>::new();
    info!("openai client initialized.");
    // --- End OpenAI Client Setup ---

    // --- Execute Tool Test ---
    info!("processing instruction using the sql query tool...");
    match sql_query_tool::process_instruction_with_sql_tool(
        &openai_client,
        &pool,
        args.instruction,
        &args.model,
    )
    .await
    {
        Ok(final_response) => {
            info!("successfully processed instruction. final ai response:");
            println!("\n🤖 lexi's final response:\n--------------------------\n{}\n--------------------------", final_response);
        }
        Err(e) => {
            error!(error = %e, "error processing instruction with sql tool");
            eprintln!("\n❌ error during tool processing: {:#}", e);
            // Optionally, return a non-zero exit code here
            // std::process::exit(1);
        }
    }
    // --- End Execute Tool Test ---

    pool.close().await;
    info!("test database pool closed.");

    Ok(())
}
