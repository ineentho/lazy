mod cli;
mod command;
mod daemon;
mod ipc;
mod runner;
mod state;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    cli::run().await
}
