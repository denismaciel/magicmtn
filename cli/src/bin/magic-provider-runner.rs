#[path = "../provider_runner.rs"]
mod provider_runner;
use clap::Parser;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    provider_runner::run(provider_runner::Args::parse()).await
}
