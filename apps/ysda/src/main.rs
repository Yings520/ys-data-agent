use clap::Parser;
use ysda::{
    bootstrap::bootstrap,
    cli::{Cli, dispatch},
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ysda=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let result = match bootstrap().await {
        Ok(dependencies) => dispatch(cli, dependencies).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("error:{}", error.code());
        std::process::exit(1);
    }
}
