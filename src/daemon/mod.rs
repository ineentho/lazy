use anyhow::{Context, Result, anyhow};
use std::{
    collections::HashMap, fs::File, io::BufReader as StdBufReader, net::SocketAddr, path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, UnixListener, UnixStream},
    sync::{Mutex, mpsc, oneshot},
    time::{Duration, timeout},
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer},
    },
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
    pub route_host: Option<String>,
    pub tls: Option<TlsConfig>,
}

#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Clone)]
struct Registry {
    suffix: String,
    listen: SocketAddr,
    route_host: Option<String>,
    tls_enabled: bool,
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
        listen: config.listen,
        route_host: config.route_host.clone(),
        tls_enabled: config.tls.is_some(),
        services: Arc::new(Mutex::new(HashMap::new())),
    };

    let control_listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("could not bind {}", socket_path.display()))?;
    let proxy_listener = TcpListener::bind(config.listen).await?;
    let tls_acceptor = config.tls.map(load_tls_acceptor).transpose()?.map(Arc::new);

    let scheme = if tls_acceptor.is_some() {
        "https"
    } else {
        "http"
    };
    println!(
        "lazy proxy listening on {}://{} with suffix {:?}",
        scheme, config.listen, config.suffix
    );
    if let Some(route_host) = &config.route_host {
        println!("path routing host: {route_host}");
    }
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
        let tls_acceptor = tls_acceptor.clone();
        tokio::spawn(async move {
            match tls_acceptor {
                Some(acceptor) => match acceptor.accept(stream).await {
                    Ok(stream) => {
                        if let Err(err) = handle_proxy(stream, registry).await {
                            eprintln!("proxy error: {err:#}");
                        }
                    }
                    Err(err) => eprintln!("tls error: {err:#}"),
                },
                None => {
                    if let Err(err) = handle_proxy(stream, registry).await {
                        eprintln!("proxy error: {err:#}");
                    }
                }
            }
        });
    }
}

fn load_tls_acceptor(config: TlsConfig) -> Result<TlsAcceptor> {
    let certs = load_certs(&config.cert)?;
    let key = load_private_key(&config.key)?;
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("could not build TLS server config")?;
    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

fn load_certs(path: &PathBuf) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path)
        .with_context(|| format!("could not open TLS certificate {}", path.display()))?;
    let mut reader = StdBufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("could not read PEM certificates from {}", path.display()))?;
    if certs.is_empty() {
        return Err(anyhow!("no certificates found in {}", path.display()));
    }
    Ok(certs)
}

fn load_private_key(path: &PathBuf) -> Result<PrivateKeyDer<'static>> {
    let file =
        File::open(path).with_context(|| format!("could not open TLS key {}", path.display()))?;
    let mut reader = StdBufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("could not read PEM private key from {}", path.display()))?
        .ok_or_else(|| anyhow!("no private key found in {}", path.display()))
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
                .then(|| registry.url_for_service(&register.name));

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

async fn handle_proxy<S>(mut inbound: S, registry: Registry) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
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
    let route = registry
        .route_for_request(&host, &mut buffer)
        .await
        .ok_or_else(|| anyhow!("host {host:?} does not match a lazy route"))?;

    let port = registry.upstream_port(&route.name).await?;
    registry.start(&route.name).await?;

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

struct ProxyRoute {
    name: String,
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

        match timeout(Duration::from_secs(360), rx).await {
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

    async fn route_for_request(&self, host: &str, buffer: &mut Vec<u8>) -> Option<ProxyRoute> {
        if self
            .route_host
            .as_deref()
            .is_some_and(|route_host| host.eq_ignore_ascii_case(route_host))
        {
            let original = buffer.clone();
            if let Some(name) = rewrite_path_route(buffer) {
                if self.has_service(&name).await {
                    return Some(ProxyRoute { name });
                }
                *buffer = original;
            }

            if let Some(name) = route_name_from_referer(buffer, host) {
                if self.has_service(&name).await {
                    return Some(ProxyRoute { name });
                }
            }

            return None;
        }

        self.service_name_from_host(host)
            .map(|name| ProxyRoute { name })
    }

    async fn has_service(&self, name: &str) -> bool {
        self.services.lock().await.contains_key(name)
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
                self.url_for_service(name)
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

    fn url_for_service(&self, name: &str) -> String {
        if let Some(route_host) = &self.route_host {
            let port = self.listen.port();
            let scheme = if self.tls_enabled { "https" } else { "http" };
            let default_port = if self.tls_enabled { 443 } else { 80 };
            if port == default_port {
                return format!("{}://{}/{}/", scheme, route_host, name);
            }
            return format!("{}://{}:{}/{}/", scheme, route_host, port, name);
        }

        let port = self.listen.port();
        let scheme = if self.tls_enabled { "https" } else { "http" };
        let default_port = if self.tls_enabled { 443 } else { 80 };
        if port == default_port {
            format!("{}://{}{}", scheme, name, self.suffix)
        } else {
            format!("{}://{}{}:{}", scheme, name, self.suffix, port)
        }
    }
}

fn rewrite_path_route(buffer: &mut Vec<u8>) -> Option<String> {
    let request = std::str::from_utf8(buffer).ok()?;
    let line_end = request.find("\r\n").or_else(|| request.find('\n'))?;
    let first_line = &request[..line_end];
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() || !target.starts_with('/') {
        return None;
    }

    let without_slash = &target[1..];
    let split_index = without_slash
        .find(['/', '?'])
        .unwrap_or(without_slash.len());
    let name = &without_slash[..split_index];
    if name.is_empty() {
        return None;
    }
    let name = name.to_string();

    let rest = &without_slash[split_index..];
    let rewritten_target = if rest.is_empty() {
        "/".to_string()
    } else if rest.starts_with('?') {
        format!("/{rest}")
    } else {
        rest.to_string()
    };
    let rewritten_first_line = format!("{method} {rewritten_target} {version}");

    buffer.splice(0..line_end, rewritten_first_line.bytes());
    Some(name)
}

fn route_name_from_referer(buffer: &[u8], route_host: &str) -> Option<String> {
    let referer = parse_header(buffer, "referer")?;
    let after_scheme = referer
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(referer);
    let (authority, path) = after_scheme.split_once('/')?;
    let host = authority.split(':').next().unwrap_or(authority);
    if !host.eq_ignore_ascii_case(route_host) {
        return None;
    }
    let name = path.split(['/', '?']).next()?;
    (!name.is_empty()).then(|| name.to_string())
}

fn parse_header<'a>(buffer: &'a [u8], header: &str) -> Option<&'a str> {
    let request = std::str::from_utf8(buffer).ok()?;
    for line in request.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case(header) {
            return Some(value.trim());
        }
    }
    None
}
