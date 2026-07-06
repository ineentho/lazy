use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use crate::{daemon, ipc, runner};

#[derive(Parser)]
#[command(name = "lazy")]
#[command(about = "On-demand dev process activation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Proxy(ProxyArgs),
    Http(HttpArgs),
    Worker(WorkerArgs),
    Status,
    Start(ServiceArgs),
    Stop(ServiceArgs),
}

#[derive(Args)]
struct ProxyArgs {
    #[arg(long, default_value = ".localhost")]
    suffix: String,

    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,
}

#[derive(Args)]
struct HttpArgs {
    name: String,

    #[arg(long)]
    upstream_port: Option<u16>,

    #[arg(long)]
    framework: Option<String>,

    #[arg(long, default_value = "4000")]
    port_range_start: u16,

    #[arg(long, default_value = "4999")]
    port_range_end: u16,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[derive(Args)]
struct WorkerArgs {
    name: String,

    #[arg(long = "while")]
    active_while: Vec<String>,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[derive(Args)]
struct ServiceArgs {
    name: String,
}

pub async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Proxy(args) => {
            daemon::run(daemon::Config {
                suffix: args.suffix,
                listen: args.listen.parse()?,
            })
            .await
        }
        Command::Http(args) => {
            runner::run_http(runner::HttpConfig {
                name: args.name,
                command: args.command,
                upstream_port: args.upstream_port,
                framework: args.framework,
                port_range_start: args.port_range_start,
                port_range_end: args.port_range_end,
            })
            .await
        }
        Command::Worker(args) => {
            runner::run_worker(runner::WorkerConfig {
                name: args.name,
                command: args.command,
                active_while: args.active_while,
            })
            .await
        }
        Command::Status => {
            let response = ipc::request(ipc::ClientRequest::Status).await?;
            println!("{response}");
            Ok(())
        }
        Command::Start(args) => {
            let response = ipc::request(ipc::ClientRequest::Start { name: args.name }).await?;
            println!("{response}");
            Ok(())
        }
        Command::Stop(args) => {
            let response = ipc::request(ipc::ClientRequest::Stop { name: args.name }).await?;
            println!("{response}");
            Ok(())
        }
    }
}
