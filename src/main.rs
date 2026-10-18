use anyhow::Context;
use clap::{Parser, Subcommand};
use lllmmm::models::config::Config;
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(
        short,
        long,
        help = "Configuration file (default: $XDG_CONFIG_HOME/lllmmm/lllmmm.toml or ~/.config/lllmmm/lllmmm.toml)"
    )]
    config: Option<PathBuf>,
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
    let path = cli.config.map(Ok).unwrap_or_else(default_config_path)?;
    let config = Arc::new(
        Config::load(&path).with_context(|| format!("load configuration {}", path.display()))?,
    );
    if matches!(cli.command, Some(Action::Check)) {
        println!("configuration valid: {} models", config.model_ids().count());
        return Ok(());
    }
    lllmmm::serve(config).await
}

fn default_config_path() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .context("set an absolute XDG_CONFIG_HOME or HOME, or specify --config")?;
    Ok(base.join("lllmmm/lllmmm.toml"))
}
