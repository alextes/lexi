use clap::Parser;

/// A simple CLI to test AI tool usage.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Instruction to give to the AI
    #[clap(short, long)]
    instruction: String,
}

fn main() {
    let args = Args::parse();
    println!("Hello, world! Your instruction was: {}", args.instruction);
    // Later, this will involve setting up the AI client and calling a function
    // from the library to process the instruction with a specific tool.
}
