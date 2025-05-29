//! ai tool cli
//!
//! this cli is used to test tools that we provide to the ai.

use clap::Parser;
use eyre::Result;
use lexi::log;

/// a cli to test ai tool usage.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct CliArgs {
    #[clap(subcommand)]
    command: Option<Commands>, // Make command optional if no commands are left, or add a placeholder
}

#[derive(Parser, Debug)]
enum Commands {
    // SqlSelectTool command removed
    // Add other tool commands here in the future if needed
    // ExamplePlaceholder {
    //     #[clap(long)]
    //     some_arg: String,
    // },
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    dotenv::dotenv().ok();

    log::init();

    let cli = CliArgs::parse();

    if let Some(command) = cli.command {
        match command {
            // Match arms for future commands would go here
            // Commands::ExamplePlaceholder { some_arg } => {
            //     println!("Example placeholder command with: {}", some_arg);
            // }
        }
    } else {
        println!("ai_tool_cli: no command given. see --help for options.");
        // Optionally, print help here if no command is given and Commands is not Option.
        // CliArgs::command().display_help(); // This requires clap features or different structure
    }

    Ok(())
}
