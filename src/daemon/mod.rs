use anyhow::{Context, Result, anyhow};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, UnixListener, UnixStream},
    sync::{Mutex, mpsc, oneshot},
    time::{Duration, timeout},
};

use crate::{
    ipc::{
        self, ClientRequest, DaemonMessage, ProcessKind, Register, RunnerMessage, SocketMessage,
    },
    state,
};

#[derive(Debug, Clone)]
pub struct Config {
    pub suffix: String,
    pub listen: SocketAddr,
}

#[derive(Clone)]
struct Registry {
    suffix: String,
    services: Arc<Mutex<HashMap<String, Service>>>,
}

struct Service {
    register: Register,
    state: ServiceState,
    control: mpsc::Sender<DaemonMessage>,
    waiters: Vec<oneshot::Sender<Result<(), String>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceState {
    Dormant,
    Starting,
    Ready,
    Failed,
}

pub async fn run(config: Config) -> Result<()> {
    let socket_path = state::socket_path()?;
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    let registry = Registry {
        suffix: config.suffix.clone(),
        services: Arc::new(Mutex::new(HashMap::new())),
    };

    let control_listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("could not bind {}", socket_path.display()))?;
    let proxy_listener = TcpListener::bind(config.listen).await?;

    println!(
        "lazy proxy listening on http://{} with suffix {:?}",
        config.listen, config.suffix
    );
    println!("control socket: {}", socket_path.display());

    let control_registry = registry.clone();
    tokio::spawn(async move {
        loop {
            match control_listener.accept().await {
                Ok((stream, _)) => {
                    let registry = control_registry.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_control(stream, registry).await {
                            eprintln!("control error: {err:#}");
                        }
                    });
                }
                Err(err) => eprintln!("control accept error: {err}"),
            }
        }
    });

    loop {
        let (stream, _) = proxy_listener.accept().await?;
        let registry = registry.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_proxy(stream, registry).await {
                eprintln!("proxy error: {err:#}");
            }
        });
    }
}

async fn handle_control(stream: UnixStream, registry: Registry) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let Some(message) = ipc::read_json::<SocketMessage>(&mut reader).await? else {
        return Ok(());
    };

    match message {
        SocketMessage::RunnerRegister { register } => {
            let (tx, mut rx) = mpsc::channel::<DaemonMessage>(16);
            let url = (register.kind == ProcessKind::Http)
                .then(|| format!("http://{}{}", register.name, registry.suffix));

            registry.register(register.clone(), tx).await?;
            let mut stream = write_half.reunite(reader.into_inner())?;
            ipc::send_json(&mut stream, &DaemonMessage::Registered { url }).await?;
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let writer = tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    if ipc::send_json(&mut write_half, &message).await.is_err() {
                        break;
                    }
                }
            });

            loop {
                let Some(message) = ipc::read_json::<RunnerMessage>(&mut reader).await? else {
                    break;
                };
                registry.apply_runner_message(message).await;
            }

            writer.abort();
        }
        SocketMessage::Client { request } => {
            let response = handle_client(request, registry).await;
            write_half.write_all(response.as_bytes()).await?;
        }
    }
    Ok(())
}

async fn handle_client(request: ClientRequest, registry: Registry) -> String {
    match request {
        ClientRequest::Status => registry.status().await,
        ClientRequest::Start { name } => match registry.start(&name).await {
            Ok(()) => format!("{name}: ready\n"),
            Err(err) => format!("{name}: {err:#}\n"),
        },
        ClientRequest::Stop { name } => match registry.stop(&name).await {
            Ok(()) => format!("{name}: stopping\n"),
            Err(err) => format!("{name}: {err:#}\n"),
        },
    }
}

async fn handle_proxy(mut inbound: TcpStream, registry: Registry) -> Result<()> {
    let mut buffer = Vec::with_capacity(8192);
    let mut chunk = [0; 1024];
    loop {
        let n = inbound.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") || buffer.len() > 64 * 1024 {
            break;
        }
    }

    let host = parse_host(&buffer).ok_or_else(|| anyhow!("request missing Host header"))?;
    let name = registry
        .service_name_from_host(&host)
        .ok_or_else(|| anyhow!("host {host:?} does not match suffix {:?}", registry.suffix))?;

    let port = registry.upstream_port(&name).await?;
    registry.start(&name).await?;

    let mut upstream = TcpStream::connect(("127.0.0.1", port)).await?;
    upstream.write_all(&buffer).await?;
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut upstream).await;
    Ok(())
}

fn parse_host(buffer: &[u8]) -> Option<String> {
    let request = std::str::from_utf8(buffer).ok()?;
    for line in request.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("host") {
            return Some(
                value
                    .trim()
                    .split(':')
                    .next()
                    .unwrap_or(value.trim())
                    .to_string(),
            );
        }
    }
    None
}

impl Registry {
    async fn register(
        &self,
        register: Register,
        control: mpsc::Sender<DaemonMessage>,
    ) -> Result<()> {
        let mut services = self.services.lock().await;
        let name = register.name.clone();
        services.insert(
            name,
            Service {
                register,
                state: ServiceState::Dormant,
                control,
                waiters: Vec::new(),
            },
        );
        Ok(())
    }

    async fn apply_runner_message(&self, message: RunnerMessage) {
        let (name, state, result) = match message {
            RunnerMessage::Ready { name } => (name, ServiceState::Ready, Ok(())),
            RunnerMessage::Stopped { name } => {
                (name, ServiceState::Dormant, Err("stopped".to_string()))
            }
            RunnerMessage::Failed { name, error } => (name, ServiceState::Failed, Err(error)),
            RunnerMessage::Register(_) => return,
        };

        let mut services = self.services.lock().await;
        if let Some(service) = services.get_mut(&name) {
            service.state = state;
            let waiters = std::mem::take(&mut service.waiters);
            for waiter in waiters {
                let _ = waiter.send(result.clone());
            }
        }
    }

    async fn start(&self, name: &str) -> Result<()> {
        let rx = {
            let mut services = self.services.lock().await;
            let service = services
                .get_mut(name)
                .ok_or_else(|| anyhow!("service not registered"))?;

            match service.state {
                ServiceState::Ready => return Ok(()),
                ServiceState::Starting => {
                    let (tx, rx) = oneshot::channel();
                    service.waiters.push(tx);
                    rx
                }
                ServiceState::Dormant | ServiceState::Failed => {
                    let (tx, rx) = oneshot::channel();
                    service.waiters.push(tx);
                    service.state = ServiceState::Starting;
                    service.control.try_send(DaemonMessage::Start)?;
                    rx
                }
            }
        };

        match timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(err))) => Err(anyhow!(err)),
            Ok(Err(_)) => Err(anyhow!("runner disconnected")),
            Err(_) => Err(anyhow!("timed out waiting for service")),
        }
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let services = self.services.lock().await;
        let service = services
            .get(name)
            .ok_or_else(|| anyhow!("service not registered"))?;
        service.control.send(DaemonMessage::Stop).await?;
        Ok(())
    }

    async fn upstream_port(&self, name: &str) -> Result<u16> {
        let services = self.services.lock().await;
        services
            .get(name)
            .and_then(|service| service.register.upstream_port)
            .ok_or_else(|| anyhow!("service {name:?} has no upstream port"))
    }

    fn service_name_from_host(&self, host: &str) -> Option<String> {
        host.strip_suffix(&self.suffix)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    }

    async fn status(&self) -> String {
        let services = self.services.lock().await;
        if services.is_empty() {
            return "no services registered\n".to_string();
        }

        let mut rows = vec!["NAME\tKIND\tSTATE\tURL\tUPSTREAM".to_string()];
        for (name, service) in services.iter() {
            let kind = match service.register.kind {
                ProcessKind::Http => "http",
                ProcessKind::Worker => "worker",
            };
            let state = match service.state {
                ServiceState::Dormant => "dormant",
                ServiceState::Starting => "starting",
                ServiceState::Ready => "ready",
                ServiceState::Failed => "failed",
            };
            let url = if service.register.kind == ProcessKind::Http {
                format!("http://{}{}", name, self.suffix)
            } else {
                "-".to_string()
            };
            let upstream = service
                .register
                .upstream_port
                .map(|p| format!("127.0.0.1:{p}"))
                .unwrap_or_else(|| "-".to_string());
            rows.push(format!("{name}\t{kind}\t{state}\t{url}\t{upstream}"));
        }
        rows.push(String::new());
        rows.join("\n")
    }
}
