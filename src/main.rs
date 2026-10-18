use clap::{Parser, Subcommand};
use lllmmm::models::config::Config;
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(short, long, default_value = "lllmmm.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    Serve,
    Check,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lllmmm=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let config = Arc::new(Config::load(&cli.config)?);
    if matches!(cli.command, Some(Action::Check)) {
        println!("configuration valid: {} models", config.model_ids().count());
        return Ok(());
    }
    lllmmm::serve(config).await
}
