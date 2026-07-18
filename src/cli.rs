use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::{net::Ipv4Addr, path::PathBuf, time::Duration};

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
    /// Host suffix appended to service names (defaults to .localhost).
    #[arg(long, conflicts_with_all = ["xip_domain", "xip_ip"])]
    suffix: Option<String>,

    /// Authoritative xip-style DNS zone, for example xip.example.com.
    #[arg(
        long,
        requires = "xip_ip",
        conflicts_with_all = ["suffix", "route_host"]
    )]
    xip_domain: Option<String>,

    /// IPv4 address encoded into each xip hostname.
    #[arg(
        long,
        requires = "xip_domain",
        conflicts_with_all = ["suffix", "route_host"]
    )]
    xip_ip: Option<Ipv4Addr>,

    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,

    /// Single host used by the legacy path-prefix routing mode.
    #[arg(long)]
    route_host: Option<String>,

    /// PEM certificate chain used to terminate TLS.
    #[arg(long, requires = "key")]
    cert: Option<PathBuf>,

    /// PEM private key used to terminate TLS.
    #[arg(long, requires = "cert")]
    key: Option<PathBuf>,
}

#[derive(Args)]
struct HttpArgs {
    name: String,

    /// How long to wait for the lazy daemon before failing.
    #[arg(
        long,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    daemon_timeout: Option<u64>,

    #[arg(long)]
    upstream_port: Option<u16>,

    #[arg(long)]
    framework: Option<String>,

    #[arg(long)]
    cwd: Option<PathBuf>,

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

    /// How long to wait for the lazy daemon before failing.
    #[arg(
        long,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    daemon_timeout: Option<u64>,

    #[arg(long = "while")]
    active_while: Vec<String>,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[derive(Args)]
struct ServiceArgs {
    name: String,
}

fn host_routing(args: &ProxyArgs) -> Result<daemon::HostRouting> {
    match (&args.xip_domain, args.xip_ip) {
        (Some(domain), Some(ip)) => daemon::HostRouting::xip(domain, ip),
        _ => Ok(daemon::HostRouting::Suffix(
            args.suffix
                .clone()
                .unwrap_or_else(|| ".localhost".to_string()),
        )),
    }
}

pub async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Proxy(args) => {
            daemon::run(daemon::Config {
                host_routing: host_routing(&args)?,
                listen: args.listen.parse()?,
                route_host: args.route_host,
                tls: match (args.cert, args.key) {
                    (Some(cert), Some(key)) => Some(daemon::TlsConfig { cert, key }),
                    _ => None,
                },
            })
            .await
        }
        Command::Http(args) => {
            runner::run_http(runner::HttpConfig {
                name: args.name,
                command: args.command,
                daemon_timeout: args.daemon_timeout.map(Duration::from_secs),
                upstream_port: args.upstream_port,
                framework: args.framework,
                cwd: args.cwd,
                port_range_start: args.port_range_start,
                port_range_end: args.port_range_end,
            })
            .await
        }
        Command::Worker(args) => {
            runner::run_worker(runner::WorkerConfig {
                name: args.name,
                command: args.command,
                daemon_timeout: args.daemon_timeout.map(Duration::from_secs),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_args(arguments: &[&str]) -> ProxyArgs {
        let cli = Cli::try_parse_from(
            ["lazy", "proxy"]
                .into_iter()
                .chain(arguments.iter().copied()),
        )
        .unwrap();
        let Command::Proxy(args) = cli.command else {
            panic!("expected proxy command");
        };
        args
    }

    #[test]
    fn proxy_defaults_to_localhost_suffix_routing() {
        let routing = host_routing(&proxy_args(&[])).unwrap();
        assert_eq!(
            routing,
            daemon::HostRouting::Suffix(".localhost".to_string())
        );
    }

    #[test]
    fn proxy_builds_xip_routing_from_domain_and_ip() {
        let args = proxy_args(&["--xip-domain", "XIP.EXAMPLE.COM.", "--xip-ip", "192.0.2.10"]);
        assert!(matches!(
            host_routing(&args).unwrap(),
            daemon::HostRouting::Xip(_)
        ));
    }

    #[test]
    fn xip_domain_and_ip_are_required_together() {
        assert!(Cli::try_parse_from(["lazy", "proxy", "--xip-domain", "xip.example.com"]).is_err());
        assert!(Cli::try_parse_from(["lazy", "proxy", "--xip-ip", "192.0.2.10"]).is_err());
    }

    #[test]
    fn xip_routing_conflicts_with_suffix_and_path_routing() {
        assert!(
            Cli::try_parse_from([
                "lazy",
                "proxy",
                "--xip-domain",
                "xip.example.com",
                "--xip-ip",
                "192.0.2.10",
                "--suffix",
                ".localhost",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "lazy",
                "proxy",
                "--xip-domain",
                "xip.example.com",
                "--xip-ip",
                "192.0.2.10",
                "--route-host",
                "node.example.com",
            ])
            .is_err()
        );
    }

    #[test]
    fn daemon_timeout_is_optional_for_http_and_worker() {
        let http = Cli::try_parse_from(["lazy", "http", "web", "--", "echo", "web"]).unwrap();
        let Command::Http(http) = http.command else {
            panic!("expected http command");
        };
        assert_eq!(http.daemon_timeout, None);

        let worker = Cli::try_parse_from(["lazy", "worker", "jobs", "--", "echo", "jobs"]).unwrap();
        let Command::Worker(worker) = worker.command else {
            panic!("expected worker command");
        };
        assert_eq!(worker.daemon_timeout, None);
    }

    #[test]
    fn daemon_timeout_is_parsed_for_http_and_worker() {
        let http = Cli::try_parse_from([
            "lazy",
            "http",
            "web",
            "--daemon-timeout",
            "10",
            "--",
            "echo",
            "web",
        ])
        .unwrap();
        let Command::Http(http) = http.command else {
            panic!("expected http command");
        };
        assert_eq!(http.daemon_timeout, Some(10));

        let worker = Cli::try_parse_from([
            "lazy",
            "worker",
            "jobs",
            "--daemon-timeout",
            "10",
            "--",
            "echo",
            "jobs",
        ])
        .unwrap();
        let Command::Worker(worker) = worker.command else {
            panic!("expected worker command");
        };
        assert_eq!(worker.daemon_timeout, Some(10));
    }

    #[test]
    fn daemon_timeout_must_be_positive() {
        assert!(
            Cli::try_parse_from([
                "lazy",
                "http",
                "web",
                "--daemon-timeout",
                "0",
                "--",
                "echo",
                "web",
            ])
            .is_err()
        );
    }
}
