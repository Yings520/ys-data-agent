use clap::Parser;
use ysda::cli::{Cli, dispatch};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(error) = dispatch(cli).await {
        eprintln!("error:{error}");
        std::process::exit(1);
    }
}
