use anyhow::{Context, Result, anyhow};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde::Serialize;
use std::{collections::HashMap, future::Future, net::Ipv4Addr, path::PathBuf, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixListener, UnixStream},
    sync::{Mutex, mpsc, oneshot},
    time::{Duration, timeout},
};
use tokio_rustls::{TlsAcceptor, rustls::ServerConfig};

use crate::{
    command::ports,
    ipc::{
        self, ClientRequest, DaemonMessage, PortRequest, ProcessKind, Register, RunnerMessage,
        SocketMessage,
    },
    listener, state,
};

#[derive(Debug, Clone)]
pub struct Config {
    pub host_routing: HostRouting,
    pub listener: listener::Source,
    pub public_port: Option<u16>,
    pub route_host: Option<String>,
    pub tls: Option<TlsConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRouting {
    Suffix(String),
    Xip(XipRouting),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XipRouting {
    domain: String,
    url_ip: Ipv4Addr,
}

impl HostRouting {
    pub fn xip(domain: &str, url_ip: Ipv4Addr) -> Result<Self> {
        let domain = normalize_domain(domain)?;
        Ok(Self::Xip(XipRouting { domain, url_ip }))
    }

    fn hostname_for_service(&self, name: &str) -> Result<String> {
        match self {
            Self::Suffix(suffix) => {
                if name.is_empty() {
                    return Err(anyhow!("service name must not be empty"));
                }
                Ok(format!("{name}{suffix}"))
            }
            Self::Xip(config) => {
                validate_dns_label(name, "service name")?;
                let encoded_ip = config.url_ip.to_string().replace('.', "-");
                let first_label = format!("{name}-{encoded_ip}");
                if first_label.len() > 63 {
                    return Err(anyhow!(
                        "service name is too long for an xip hostname (the service and encoded IP must fit in one 63-character DNS label)"
                    ));
                }
                let hostname = format!("{first_label}.{}", config.domain);
                if hostname.len() > 253 {
                    return Err(anyhow!("generated xip hostname exceeds 253 characters"));
                }
                Ok(hostname)
            }
        }
    }

    fn service_name_from_host(&self, host: &str) -> Option<String> {
        match self {
            Self::Suffix(suffix) => strip_suffix_ascii_case(host, suffix),
            Self::Xip(config) => xip_service_name_from_host(host, &config.domain),
        }
        .filter(|name| !name.is_empty())
    }

    fn description(&self) -> String {
        match self {
            Self::Suffix(suffix) => format!("suffix {suffix:?}"),
            Self::Xip(config) => {
                format!(
                    "xip domain {:?} generating URLs with address {}",
                    config.domain, config.url_ip
                )
            }
        }
    }

    fn status_hostname(&self) -> Result<String> {
        match self {
            Self::Suffix(suffix) => {
                let hostname = suffix.trim_start_matches('.');
                if hostname.is_empty() {
                    return Err(anyhow!("suffix has no bare hostname"));
                }
                Ok(hostname.to_ascii_lowercase())
            }
            Self::Xip(config) => {
                let encoded_ip = config.url_ip.to_string().replace('.', "-");
                Ok(format!("{encoded_ip}.{}", config.domain))
            }
        }
    }

    fn is_status_host(&self, host: &str) -> bool {
        match self {
            Self::Suffix(_) => self
                .status_hostname()
                .is_ok_and(|status_host| host.eq_ignore_ascii_case(&status_host)),
            Self::Xip(config) => {
                xip_service_name_from_host(host, &config.domain).is_some_and(|name| name.is_empty())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StatusRow {
    name: String,
    kind: String,
    state: String,
    url: String,
    upstream: String,
    detail: String,
}

const STATUS_HTML_SHELL: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>lazy status</title>
<style>
body{font-family:ui-monospace,monospace;margin:2rem}
table{border-collapse:collapse;width:100%}
th,td{text-align:left;padding:.5em;border-bottom:1px solid #ddd}
th{background:#f5f5f5}
.detail{font-style:italic;color:#666}
.stop-link{appearance:none;border:0;background:none;padding:0;font:inherit;color:#06c;text-decoration:underline;cursor:pointer}
.stop-link:focus-visible{outline:2px solid currentColor;outline-offset:2px}
.stop-link:disabled{color:#888;text-decoration:none;cursor:default}
.error{color:#c00}
.loading{color:#666}
</style>
</head>
<body>
<h1>lazy services</h1>
<p id="action-error" class="error" hidden></p>
<div id="root"><p class="loading">loading&hellip;</p></div>
<script>
(function(){
  const root = document.getElementById("root");
  const actionError = document.getElementById("action-error");
  const stopping = new Set();

  const showMessage = (message, className) => {
    const p = document.createElement("p");
    if (className) p.className = className;
    p.textContent = message;
    root.replaceChildren(p);
  };

  const showActionError = message => {
    actionError.textContent = message;
    actionError.hidden = !message;
  };

  const refresh = async () => {
    try {
      const response = await fetch("/api/status");
      if (!response.ok) throw new Error(response.statusText);
      const services = await response.json();
      if (services.length === 0) {
        showMessage("no services registered");
        return;
      }

      const table = document.createElement("table");
      const header = document.createElement("tr");
      ["NAME","KIND","STATE","URL","UPSTREAM","DETAIL","ACTION"].forEach(label => {
        const th = document.createElement("th");
        th.textContent = label;
        header.appendChild(th);
      });
      table.appendChild(header);

      services.forEach(service => {
        if (service.state === "dormant" || service.state === "failed") {
          stopping.delete(service.name);
        }
        const row = document.createElement("tr");
        ["name","kind","state","url","upstream","detail"].forEach(field => {
          const td = document.createElement("td");
          if (field === "url" && service.url !== "-") {
            const link = document.createElement("a");
            link.setAttribute("href", service.url);
            link.textContent = service.url;
            td.appendChild(link);
          } else {
            td.textContent = service[field];
          }
          if (field === "detail") td.className = "detail";
          row.appendChild(td);
        });

        const action = document.createElement("td");
        const button = document.createElement("button");
        const canStop = service.state === "starting" || service.state === "ready";
        button.type = "button";
        button.className = "stop-link";
        button.disabled = !canStop || stopping.has(service.name);
        button.textContent = "Stop";
        if (canStop) {
          button.addEventListener("click", async () => {
            stopping.add(service.name);
            button.disabled = true;
            showActionError("");
            try {
              const response = await fetch("/api/stop", {
                method: "POST",
                headers: {
                  "Content-Type": "application/json",
                  "X-Lazy-Service": service.name
                },
                body: "{}"
              });
              if (!response.ok) {
                const body = await response.json().catch(() => ({}));
                throw new Error(body.error || response.statusText);
              }
              await refresh();
            } catch (error) {
              stopping.delete(service.name);
              button.disabled = false;
              showActionError("stop failed: " + error.message);
            }
          });
        }
        action.appendChild(button);
        row.appendChild(action);
        table.appendChild(row);
      });
      root.replaceChildren(table);
    } catch (error) {
      showMessage("fetch error: " + error.message, "error");
    }
  };

  const poll = async () => {
    await refresh();
    setTimeout(poll, 2000);
  };
  poll();
})();
</script>
</body>
</html>"#;

#[derive(Clone)]
struct Registry {
    host_routing: HostRouting,
    public_port: u16,
    route_host: Option<String>,
    tls_enabled: bool,
    services: Arc<Mutex<HashMap<String, Service>>>,
}

struct Service {
    register: Register,
    active_port: Option<u16>,
    state: ServiceState,
    last_error: Option<String>,
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

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_HEADER_BYTES: usize = 64 * 1024;

pub async fn run(config: Config) -> Result<()> {
    let socket_path = state::socket_path()?;
    state::remove_stale_control_socket(&socket_path)?;

    let std_proxy_listener = listener::acquire(&config.listener)?;
    let listen_address = std_proxy_listener.local_addr()?;
    let public_port = config.public_port.unwrap_or(listen_address.port());
    let proxy_listener = TcpListener::from_std(std_proxy_listener)?;
    let tls_acceptor = config.tls.map(load_tls_acceptor).transpose()?.map(Arc::new);

    let registry = Registry {
        host_routing: config.host_routing.clone(),
        public_port,
        route_host: config.route_host.clone(),
        tls_enabled: tls_acceptor.is_some(),
        services: Arc::new(Mutex::new(HashMap::new())),
    };

    let control_listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("could not bind {}", socket_path.display()))?;
    let _control_socket_guard = state::ControlSocketGuard::new(socket_path.clone())?;
    state::secure_control_socket(&socket_path)?;

    let scheme = if tls_acceptor.is_some() {
        "https"
    } else {
        "http"
    };
    println!(
        "lazy proxy listening on {}://{} with {}",
        scheme,
        listen_address,
        config.host_routing.description()
    );
    if public_port != listen_address.port() {
        println!("public service port: {public_port}");
    }
    if let Some(route_host) = &config.route_host {
        println!("path routing host: {route_host}");
    }
    println!("control socket: {}", socket_path.display());
    if !listen_address.ip().is_loopback() {
        eprintln!(
            "WARNING: network clients are not authenticated; restrict {} with a firewall or tailnet ACL",
            listen_address
        );
    }

    tokio::select! {
        result = serve_control(&control_listener, registry.clone()) => result,
        result = serve_proxy(&proxy_listener, registry, tls_acceptor) => result,
        result = shutdown_signal() => result,
    }
}

async fn serve_control(listener: &UnixListener, registry: Registry) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let registry = registry.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_control(stream, registry).await {
                eprintln!("control error: {err:#}");
            }
        });
    }
}

async fn serve_proxy(
    listener: &TcpListener,
    registry: Registry,
    tls_acceptor: Option<Arc<TlsAcceptor>>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let registry = registry.clone();
        let tls_acceptor = tls_acceptor.clone();
        tokio::spawn(async move {
            match tls_acceptor {
                Some(acceptor) => match run_with_timeout(
                    TLS_HANDSHAKE_TIMEOUT,
                    "TLS handshake",
                    acceptor.accept(stream),
                )
                .await
                {
                    Ok(Ok(stream)) => {
                        let _ = handle_proxy(stream, registry).await;
                    }
                    Ok(Err(err)) => eprintln!("tls error: {err:#}"),
                    Err(err) => eprintln!("tls error: {err:#}"),
                },
                None => {
                    let _ = handle_proxy(stream, registry).await;
                }
            }
        });
    }
}

async fn shutdown_signal() -> Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {},
    }
    Ok(())
}

async fn run_with_timeout<T>(
    duration: Duration,
    operation: &str,
    future: impl Future<Output = T>,
) -> Result<T> {
    timeout(duration, future)
        .await
        .map_err(|_| anyhow!("{operation} timed out after {duration:?}"))
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
    let certs = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("could not open TLS certificate {}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("could not read PEM certificates from {}", path.display()))?;
    if certs.is_empty() {
        return Err(anyhow!("no certificates found in {}", path.display()));
    }
    Ok(certs)
}

fn load_private_key(path: &PathBuf) -> Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path)
        .with_context(|| format!("could not read PEM private key from {}", path.display()))
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
            let name = register.name.clone();
            let url = if register.kind == ProcessKind::Http {
                Some(registry.url_for_service(&register.name)?)
            } else {
                None
            };

            if let Err(error) = registry.register(register, tx.clone()).await {
                ipc::send_json(
                    &mut write_half,
                    &DaemonMessage::Error {
                        message: error.to_string(),
                    },
                )
                .await?;
                return Ok(());
            }
            let result = async {
                let mut stream = write_half.reunite(reader.into_inner())?;
                ipc::send_json(&mut stream, &DaemonMessage::Registered { url }).await?;
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);

                tokio::spawn(async move {
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
                Ok::<(), anyhow::Error>(())
            }
            .await;

            registry.unregister(&name, &tx).await;
            result?;
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
    let Some(mut buffer) = read_request_headers(&mut inbound, REQUEST_HEADER_TIMEOUT).await? else {
        return Ok(());
    };

    let host = parse_host(&buffer).ok_or_else(|| anyhow!("request missing Host header"))?;
    let route = registry.route_for_request(&host, &mut buffer).await;
    let route = match route {
        Some(route) => route,
        None => {
            if registry
                .try_serve_status(&host, &buffer, &mut inbound)
                .await?
            {
                return Ok(());
            }
            return Err(anyhow!("host {host:?} does not match a lazy route"));
        }
    };

    registry.start(&route.name).await?;
    let port = registry.upstream_port(&route.name).await?;

    let mut upstream = run_with_timeout(
        UPSTREAM_CONNECT_TIMEOUT,
        "upstream connection",
        ports::connect_loopback(port),
    )
    .await??;
    upstream.write_all(&buffer).await?;
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut upstream).await;
    Ok(())
}

async fn read_request_headers<S>(inbound: &mut S, deadline: Duration) -> Result<Option<Vec<u8>>>
where
    S: AsyncRead + Unpin,
{
    run_with_timeout(deadline, "HTTP request headers", async {
        let mut buffer = Vec::with_capacity(8192);
        let mut chunk = [0; 1024];
        loop {
            let n = inbound.read(&mut chunk).await?;
            if n == 0 {
                return if buffer.is_empty() {
                    Ok(None)
                } else {
                    Err(anyhow!("connection closed before HTTP headers completed"))
                };
            }
            buffer.extend_from_slice(&chunk[..n]);
            if let Some(header_end) = buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            {
                if header_end > MAX_REQUEST_HEADER_BYTES {
                    return Err(anyhow!(
                        "HTTP request headers exceed {MAX_REQUEST_HEADER_BYTES} bytes"
                    ));
                }
                return Ok(Some(buffer));
            }
            if buffer.len() >= MAX_REQUEST_HEADER_BYTES {
                return Err(anyhow!(
                    "HTTP request headers exceed {MAX_REQUEST_HEADER_BYTES} bytes"
                ));
            }
        }
    })
    .await?
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
        match (register.kind, register.port_request) {
            (ProcessKind::Http, Some(PortRequest::Fixed { port })) if port > 0 => {}
            (ProcessKind::Http, Some(PortRequest::Range { start, end }))
                if start > 0 && start <= end => {}
            (ProcessKind::Http, Some(PortRequest::Fixed { .. })) => {
                return Err(anyhow!("upstream port must be greater than zero"));
            }
            (ProcessKind::Http, Some(PortRequest::Range { start, end })) => {
                return Err(anyhow!("invalid port range {start}-{end}"));
            }
            (ProcessKind::Http, None) => {
                return Err(anyhow!("HTTP service registration requires a port request"));
            }
            (ProcessKind::Worker, Some(_)) => {
                return Err(anyhow!("worker registration must not request a port"));
            }
            (ProcessKind::Worker, None) => {}
        }

        let mut services = self.services.lock().await;
        let name = register.name.clone();
        if services.contains_key(&name) {
            return Err(anyhow!("service {name:?} is already registered"));
        }
        services.insert(
            name,
            Service {
                register,
                active_port: None,
                state: ServiceState::Dormant,
                last_error: None,
                control,
                waiters: Vec::new(),
            },
        );
        Ok(())
    }

    async fn unregister(&self, name: &str, control: &mpsc::Sender<DaemonMessage>) {
        let mut services = self.services.lock().await;
        let is_current = services
            .get(name)
            .is_some_and(|service| service.control.same_channel(control));
        if !is_current {
            return;
        }
        if let Some(mut service) = services.remove(name) {
            for waiter in std::mem::take(&mut service.waiters) {
                let _ = waiter.send(Err("runner disconnected".to_string()));
            }
        }
    }

    async fn apply_runner_message(&self, message: RunnerMessage) {
        let (name, state, result, release_port, last_error) = match message {
            RunnerMessage::Ready { name } => (name, ServiceState::Ready, Ok(()), false, None),
            RunnerMessage::Stopped { name } => (
                name,
                ServiceState::Dormant,
                Err("stopped".to_string()),
                true,
                None,
            ),
            RunnerMessage::Failed { name, error } => (
                name,
                ServiceState::Failed,
                Err(error.clone()),
                true,
                Some(error),
            ),
            RunnerMessage::Register(_) => return,
        };

        let mut services = self.services.lock().await;
        if let Some(service) = services.get_mut(&name) {
            service.state = state;
            service.last_error = last_error;
            if release_port {
                service.active_port = None;
            }
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
                .get(name)
                .ok_or_else(|| anyhow!("service not registered"))?;

            match service.state {
                ServiceState::Ready => return Ok(()),
                ServiceState::Starting => {
                    let (tx, rx) = oneshot::channel();
                    let service = services.get_mut(name).unwrap();
                    service.waiters.push(tx);
                    rx
                }
                ServiceState::Dormant | ServiceState::Failed => {
                    let port = match service.register.kind {
                        ProcessKind::Http => Some(allocate_port(
                            &services,
                            service.register.port_request.unwrap(),
                        )?),
                        ProcessKind::Worker => None,
                    };
                    let service = services.get_mut(name).unwrap();
                    service.control.try_send(DaemonMessage::Start { port })?;
                    let (tx, rx) = oneshot::channel();
                    service.waiters.push(tx);
                    service.state = ServiceState::Starting;
                    service.active_port = port;
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
            .and_then(|service| service.active_port)
            .ok_or_else(|| anyhow!("service {name:?} has no active upstream port"))
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

            if let Some(name) = route_name_from_referer(buffer, host)
                && self.has_service(&name).await
            {
                return Some(ProxyRoute { name });
            }

            return None;
        }

        let mut name = self.host_routing.service_name_from_host(host)?;
        if matches!(self.host_routing, HostRouting::Xip(_)) {
            let services = self.services.lock().await;
            if let Some(registered_name) = services
                .keys()
                .filter(|registered_name| {
                    name == registered_name.as_str()
                        || name
                            .strip_suffix(registered_name.as_str())
                            .is_some_and(|prefix| prefix.ends_with('-'))
                })
                .max_by_key(|registered_name| registered_name.len())
            {
                name = registered_name.clone();
            }
        }

        Some(ProxyRoute { name })
    }

    async fn has_service(&self, name: &str) -> bool {
        self.services.lock().await.contains_key(name)
    }

    async fn collect_rows(&self) -> Vec<StatusRow> {
        let services = self.services.lock().await;
        let mut rows = Vec::with_capacity(services.len());
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
                    .unwrap_or_else(|error| format!("invalid service name: {error}"))
            } else {
                "-".to_string()
            };
            let upstream = service
                .active_port
                .map(|p| format!("127.0.0.1:{p}"))
                .unwrap_or_else(|| "-".to_string());
            let detail = service
                .last_error
                .as_deref()
                .map(sanitize_status_detail)
                .unwrap_or_else(|| "-".to_string());
            rows.push(StatusRow {
                name: name.clone(),
                kind: kind.to_string(),
                state: state.to_string(),
                url,
                upstream,
                detail,
            });
        }
        rows
    }

    async fn status(&self) -> String {
        let rows = self.collect_rows().await;
        if rows.is_empty() {
            return "no services registered\n".to_string();
        }
        let mut lines = vec!["NAME\tKIND\tSTATE\tURL\tUPSTREAM\tDETAIL".to_string()];
        for row in &rows {
            lines.push(format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                row.name, row.kind, row.state, row.url, row.upstream, row.detail
            ));
        }
        lines.push(String::new());
        lines.join("\n")
    }

    async fn status_json(&self) -> String {
        let rows = self.collect_rows().await;
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }

    fn url_for_service(&self, name: &str) -> Result<String> {
        if let Some(route_host) = &self.route_host {
            let port = self.public_port;
            let scheme = if self.tls_enabled { "https" } else { "http" };
            let default_port = if self.tls_enabled { 443 } else { 80 };
            if port == default_port {
                return Ok(format!("{}://{}/{}/", scheme, route_host, name));
            }
            return Ok(format!("{}://{}:{}/{}/", scheme, route_host, port, name));
        }

        let hostname = self.host_routing.hostname_for_service(name)?;
        let port = self.public_port;
        let scheme = if self.tls_enabled { "https" } else { "http" };
        let default_port = if self.tls_enabled { 443 } else { 80 };
        if port == default_port {
            Ok(format!("{scheme}://{hostname}"))
        } else {
            Ok(format!("{scheme}://{hostname}:{port}"))
        }
    }

    async fn try_serve_status<S: AsyncWrite + Unpin>(
        &self,
        host: &str,
        buffer: &[u8],
        stream: &mut S,
    ) -> Result<bool> {
        if !self.host_routing.is_status_host(host) {
            return Ok(false);
        }
        match request_method_path(buffer) {
            Some(("GET", "/api/status")) => {
                let body = self.status_json().await;
                write_http_response(stream, "200 OK", "application/json; charset=utf-8", &body)
                    .await?;
                Ok(true)
            }
            Some(("POST", "/api/stop")) => {
                if !parse_header(buffer, "content-type").is_some_and(|value| {
                    value
                        .split(';')
                        .next()
                        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
                }) {
                    write_http_response(
                        stream,
                        "415 Unsupported Media Type",
                        "application/json; charset=utf-8",
                        r#"{"error":"content type must be application/json"}"#,
                    )
                    .await?;
                    return Ok(true);
                }
                let Some(name) =
                    parse_header(buffer, "x-lazy-service").filter(|name| !name.is_empty())
                else {
                    write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json; charset=utf-8",
                        r#"{"error":"missing X-Lazy-Service header"}"#,
                    )
                    .await?;
                    return Ok(true);
                };
                if !self.has_service(name).await {
                    write_http_response(
                        stream,
                        "404 Not Found",
                        "application/json; charset=utf-8",
                        r#"{"error":"service not registered"}"#,
                    )
                    .await?;
                    return Ok(true);
                }
                match self.stop(name).await {
                    Ok(()) => {
                        write_http_response(
                            stream,
                            "202 Accepted",
                            "application/json; charset=utf-8",
                            r#"{"ok":true}"#,
                        )
                        .await?;
                    }
                    Err(error) => {
                        let body = serde_json::json!({ "error": error.to_string() }).to_string();
                        write_http_response(
                            stream,
                            "500 Internal Server Error",
                            "application/json; charset=utf-8",
                            &body,
                        )
                        .await?;
                    }
                }
                Ok(true)
            }
            Some(("GET", "/")) => {
                write_http_response(
                    stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    STATUS_HTML_SHELL,
                )
                .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

fn sanitize_status_detail(detail: &str) -> String {
    detail.replace(['\t', '\r', '\n'], " ")
}

fn allocate_port(services: &HashMap<String, Service>, request: PortRequest) -> Result<u16> {
    let available = |port| {
        !services
            .values()
            .any(|service| service.active_port == Some(port))
            && std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
    };

    match request {
        PortRequest::Fixed { port } if available(port) => Ok(port),
        PortRequest::Fixed { port } => Err(anyhow!("upstream port {port} is unavailable")),
        PortRequest::Range { start, end } => (start..=end)
            .find(|port| available(*port))
            .ok_or_else(|| anyhow!("no free port found in range {start}-{end}")),
    }
}

fn normalize_domain(domain: &str) -> Result<String> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return Err(anyhow!("xip domain must not be empty"));
    }
    if domain.len() > 253 {
        return Err(anyhow!("xip domain exceeds 253 characters"));
    }
    for label in domain.split('.') {
        validate_dns_label(label, "xip domain label")?;
    }
    Ok(domain)
}

fn validate_dns_label(label: &str, description: &str) -> Result<()> {
    if label.is_empty() {
        return Err(anyhow!("{description} must not be empty"));
    }
    if label.len() > 63 {
        return Err(anyhow!("{description} exceeds 63 characters"));
    }
    if !label.is_ascii() {
        return Err(anyhow!("{description} must contain only ASCII characters"));
    }
    if !label
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(anyhow!(
            "{description} must contain only lowercase letters, digits, and hyphens"
        ));
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err(anyhow!(
            "{description} must start and end with a letter or digit"
        ));
    }
    Ok(())
}

fn strip_suffix_ascii_case(value: &str, suffix: &str) -> Option<String> {
    if !value.is_ascii() || !suffix.is_ascii() || value.len() < suffix.len() {
        return None;
    }
    let split = value.len() - suffix.len();
    value[split..]
        .eq_ignore_ascii_case(suffix)
        .then(|| value[..split].to_string())
}

fn xip_service_name_from_host(host: &str, domain: &str) -> Option<String> {
    let label = strip_suffix_ascii_case(host, &format!(".{domain}"))?;
    if label.contains('.') {
        return None;
    }

    let mut parts = label.rsplitn(5, '-');
    for _ in 0..4 {
        let octet = parts.next()?;
        if !octet.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        octet.parse::<u8>().ok()?;
    }

    Some(parts.next().unwrap_or("").to_ascii_lowercase())
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

async fn write_http_response<S: AsyncWrite + Unpin>(
    stream: &mut S,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        content_type,
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

fn request_method_path(buffer: &[u8]) -> Option<(&str, &str)> {
    let request = std::str::from_utf8(buffer).ok()?;
    let line_end = request.find("\r\n").or_else(|| request.find('\n'))?;
    let first_line = &request[..line_end];
    let mut parts = first_line.split_whitespace();
    Some((parts.next()?, parts.next()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    static PORT_TEST_LOCK: Mutex<()> = Mutex::const_new(());

    fn registry(host_routing: HostRouting, port: u16, tls_enabled: bool) -> Registry {
        Registry {
            host_routing,
            public_port: port,
            route_host: None,
            tls_enabled,
            services: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn reads_a_complete_request_header() {
        let request = b"GET / HTTP/1.1\r\nHost: vite.localhost\r\n\r\n";
        let mut input = &request[..];

        let headers = read_request_headers(&mut input, Duration::from_secs(1))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(headers, request);
    }

    #[tokio::test]
    async fn rejects_oversized_request_headers() {
        let mut request = vec![b'a'; MAX_REQUEST_HEADER_BYTES + 1];
        request.extend_from_slice(b"\r\n\r\n");
        let mut input = request.as_slice();

        let error = read_request_headers(&mut input, Duration::from_secs(1))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("exceed"));
    }

    #[tokio::test]
    async fn header_limit_does_not_count_buffered_request_body() {
        let mut request = b"POST / HTTP/1.1\r\nHost: vite.localhost\r\n\r\n".to_vec();
        request.extend(std::iter::repeat_n(b'a', MAX_REQUEST_HEADER_BYTES + 1));
        let mut input = request.as_slice();

        let buffered = read_request_headers(&mut input, Duration::from_secs(1))
            .await
            .unwrap()
            .unwrap();

        assert!(buffered.starts_with(b"POST / HTTP/1.1"));
    }

    #[tokio::test]
    async fn times_out_stalled_request_headers() {
        let (mut inbound, _client) = tokio::io::duplex(64);

        let error = read_request_headers(&mut inbound, Duration::from_millis(10))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("HTTP request headers timed out"));
    }

    #[tokio::test]
    async fn operation_deadlines_cover_tls_and_upstream_setup() {
        for operation in ["TLS handshake", "upstream connection"] {
            let error = run_with_timeout(
                Duration::from_millis(10),
                operation,
                std::future::pending::<()>(),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(operation));
            assert!(error.to_string().contains("timed out"));
        }
    }

    fn free_port_range(size: u16) -> (u16, u16) {
        'range: for start in 20_000u16..60_000u16 {
            let end = start + size - 1;
            let mut listeners = Vec::new();
            for port in start..=end {
                match std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                    Ok(listener) => listeners.push(listener),
                    Err(_) => continue 'range,
                }
            }
            drop(listeners);
            return (start, end);
        }
        panic!("could not find {size} consecutive free ports for test");
    }

    async fn register_http(
        registry: &Registry,
        name: &str,
        request: PortRequest,
    ) -> (mpsc::Sender<DaemonMessage>, mpsc::Receiver<DaemonMessage>) {
        let (control, messages) = mpsc::channel(4);
        registry
            .register(
                Register {
                    name: name.to_string(),
                    kind: ProcessKind::Http,
                    port_request: Some(request),
                },
                control.clone(),
            )
            .await
            .unwrap();
        (control, messages)
    }

    async fn mark_ready(
        registry: Registry,
        name: &'static str,
        mut messages: mpsc::Receiver<DaemonMessage>,
    ) -> u16 {
        let Some(DaemonMessage::Start { port: Some(port) }) = messages.recv().await else {
            panic!("expected HTTP start message with a port");
        };
        registry
            .apply_runner_message(RunnerMessage::Ready {
                name: name.to_string(),
            })
            .await;
        port
    }

    #[tokio::test]
    async fn allocates_distinct_ports_when_services_are_started() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let (start, end) = free_port_range(2);
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (_, one_messages) =
            register_http(&registry, "one", PortRequest::Range { start, end }).await;
        let (_, two_messages) =
            register_http(&registry, "two", PortRequest::Range { start, end }).await;

        assert!(registry.upstream_port("one").await.is_err());
        let one_ready = tokio::spawn(mark_ready(registry.clone(), "one", one_messages));
        let two_ready = tokio::spawn(mark_ready(registry.clone(), "two", two_messages));
        let (one, two) = tokio::join!(registry.start("one"), registry.start("two"));
        one.unwrap();
        two.unwrap();

        let one_port = one_ready.await.unwrap();
        let two_port = two_ready.await.unwrap();
        assert_ne!(one_port, two_port);
        assert!((start..=end).contains(&one_port));
        assert!((start..=end).contains(&two_port));
    }

    #[tokio::test]
    async fn releases_automatic_port_after_service_stops() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let (port, _) = free_port_range(1);
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (_, one_messages) = register_http(
            &registry,
            "one",
            PortRequest::Range {
                start: port,
                end: port,
            },
        )
        .await;
        let (_, two_messages) = register_http(
            &registry,
            "two",
            PortRequest::Range {
                start: port,
                end: port,
            },
        )
        .await;

        let one_ready = tokio::spawn(mark_ready(registry.clone(), "one", one_messages));
        registry.start("one").await.unwrap();
        assert_eq!(one_ready.await.unwrap(), port);

        let error = registry.start("two").await.unwrap_err();
        assert!(error.to_string().contains("no free port found"));

        registry
            .apply_runner_message(RunnerMessage::Stopped {
                name: "one".to_string(),
            })
            .await;
        let two_ready = tokio::spawn(mark_ready(registry.clone(), "two", two_messages));
        registry.start("two").await.unwrap();
        assert_eq!(two_ready.await.unwrap(), port);
    }

    #[tokio::test]
    async fn explicit_port_is_checked_when_service_starts() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let (port, _) = free_port_range(1);
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (_, one_messages) = register_http(&registry, "one", PortRequest::Fixed { port }).await;
        let (_, _two_messages) = register_http(&registry, "two", PortRequest::Fixed { port }).await;

        let one_ready = tokio::spawn(mark_ready(registry.clone(), "one", one_messages));
        registry.start("one").await.unwrap();
        assert_eq!(one_ready.await.unwrap(), port);

        let error = registry.start("two").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("upstream port {port} is unavailable")
        );
    }

    #[tokio::test]
    async fn disconnect_removes_service_and_releases_waiters() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let (port, _) = free_port_range(1);
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (control, mut messages) =
            register_http(&registry, "one", PortRequest::Fixed { port }).await;

        let start_registry = registry.clone();
        let start = tokio::spawn(async move { start_registry.start("one").await });
        assert!(matches!(
            messages.recv().await,
            Some(DaemonMessage::Start { port: Some(value) }) if value == port
        ));
        registry.unregister("one", &control).await;

        let error = start.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("runner disconnected"));
        assert_eq!(registry.status().await, "no services registered\n");
    }

    #[test]
    fn xip_routing_generates_wildcard_compatible_hostname() {
        let routing = HostRouting::xip("XIP.EXAMPLE.COM.", Ipv4Addr::new(192, 0, 2, 10)).unwrap();

        assert_eq!(
            routing.hostname_for_service("vite").unwrap(),
            "vite-192-0-2-10.xip.example.com"
        );
    }

    #[test]
    fn xip_routing_extracts_service_case_insensitively() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(192, 0, 2, 10)).unwrap();

        assert_eq!(
            routing.service_name_from_host("VITE-192-0-2-10.XIP.EXAMPLE.COM"),
            Some("vite".to_string())
        );
        assert_eq!(
            routing.service_name_from_host("vite-192-0-2-11.xip.example.com"),
            Some("vite".to_string())
        );
        assert_eq!(
            routing.service_name_from_host("vite-192-0-2-256.xip.example.com"),
            None
        );
        assert_eq!(
            routing.service_name_from_host("vite-192-0-2-11.other.example.com"),
            None
        );
    }

    #[tokio::test]
    async fn xip_routing_accepts_an_ip_other_than_the_generated_url_ip() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(192, 0, 2, 10)).unwrap();
        let registry = registry(routing, 8080, false);
        register_http(&registry, "api", PortRequest::Fixed { port: 4000 }).await;

        let route = registry
            .route_for_request("api-127-0-0-1.xip.example.com", &mut Vec::new())
            .await
            .unwrap();

        assert_eq!(route.name, "api");
    }

    #[tokio::test]
    async fn xip_routing_matches_a_registered_service_after_a_prefix() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(192, 0, 2, 10)).unwrap();
        let registry = registry(routing, 8080, false);
        register_http(&registry, "api", PortRequest::Fixed { port: 4000 }).await;

        let route = registry
            .route_for_request("acme-api-192-0-2-10.xip.example.com", &mut Vec::new())
            .await
            .unwrap();

        assert_eq!(route.name, "api");
    }

    #[tokio::test]
    async fn xip_routing_prefers_the_longest_registered_suffix() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(192, 0, 2, 10)).unwrap();
        let registry = registry(routing, 8080, false);
        register_http(&registry, "api", PortRequest::Fixed { port: 4000 }).await;
        register_http(&registry, "internal-api", PortRequest::Fixed { port: 4001 }).await;

        let route = registry
            .route_for_request(
                "acme-internal-api-192-0-2-10.xip.example.com",
                &mut Vec::new(),
            )
            .await
            .unwrap();

        assert_eq!(route.name, "internal-api");
    }

    #[tokio::test]
    async fn xip_routing_prefers_an_exact_registered_name() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(192, 0, 2, 10)).unwrap();
        let registry = registry(routing, 8080, false);
        register_http(&registry, "api", PortRequest::Fixed { port: 4000 }).await;
        register_http(&registry, "acme-api", PortRequest::Fixed { port: 4001 }).await;

        let route = registry
            .route_for_request("acme-api-192-0-2-10.xip.example.com", &mut Vec::new())
            .await
            .unwrap();

        assert_eq!(route.name, "acme-api");
    }

    #[tokio::test]
    async fn xip_routing_requires_a_hyphen_before_the_registered_service() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(192, 0, 2, 10)).unwrap();
        let registry = registry(routing, 8080, false);
        register_http(&registry, "api", PortRequest::Fixed { port: 4000 }).await;

        let route = registry
            .route_for_request("myapi-192-0-2-10.xip.example.com", &mut Vec::new())
            .await
            .unwrap();

        assert_eq!(route.name, "myapi");
    }

    #[tokio::test]
    async fn wildcard_prefix_matching_is_limited_to_xip_routing() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        register_http(&registry, "api", PortRequest::Fixed { port: 4000 }).await;

        let route = registry
            .route_for_request("acme-api.localhost", &mut Vec::new())
            .await
            .unwrap();

        assert_eq!(route.name, "acme-api");
    }

    #[test]
    fn xip_urls_use_https_and_omit_the_default_port() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(192, 0, 2, 10)).unwrap();
        let default_tls = registry(routing.clone(), 443, true);
        let custom_tls = registry(routing, 18443, true);

        assert_eq!(
            default_tls.url_for_service("vite").unwrap(),
            "https://vite-192-0-2-10.xip.example.com"
        );
        assert_eq!(
            custom_tls.url_for_service("vite").unwrap(),
            "https://vite-192-0-2-10.xip.example.com:18443"
        );
    }

    #[test]
    fn localhost_suffix_routing_remains_supported() {
        let routing = HostRouting::Suffix(".localhost".to_string());
        let registry = registry(routing.clone(), 8080, false);

        assert_eq!(
            routing.service_name_from_host("VITE.LOCALHOST"),
            Some("VITE".to_string())
        );
        assert_eq!(
            registry.url_for_service("vite").unwrap(),
            "http://vite.localhost:8080"
        );
    }

    #[test]
    fn xip_routing_rejects_invalid_domains_and_service_names() {
        assert!(HostRouting::xip("bad_domain.example", Ipv4Addr::LOCALHOST).is_err());
        assert!(HostRouting::xip("xip..example.com", Ipv4Addr::LOCALHOST).is_err());

        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::LOCALHOST).unwrap();
        assert!(routing.hostname_for_service("Vite").is_err());
        assert!(routing.hostname_for_service("vite.dev").is_err());
        assert!(routing.hostname_for_service("-vite").is_err());
    }

    #[test]
    fn xip_routing_rejects_service_names_that_overflow_the_first_label() {
        let routing =
            HostRouting::xip("xip.example.com", Ipv4Addr::new(255, 255, 255, 255)).unwrap();
        let service = "a".repeat(48);

        assert!(routing.hostname_for_service(&service).is_err());
    }

    #[test]
    fn parse_host_accepts_a_port() {
        let request = b"GET / HTTP/1.1\r\nHost: vite-192-0-2-10.xip.example.com:18443\r\n\r\n";

        assert_eq!(
            parse_host(request).as_deref(),
            Some("vite-192-0-2-10.xip.example.com")
        );
    }

    // --- status page tests ---

    #[test]
    fn status_hostname_for_suffix() {
        let routing = HostRouting::Suffix(".localhost".to_string());
        assert_eq!(routing.status_hostname().unwrap(), "localhost");
    }

    #[test]
    fn status_hostname_for_suffix_without_dot() {
        let routing = HostRouting::Suffix("local".to_string());
        assert_eq!(routing.status_hostname().unwrap(), "local");
    }

    #[test]
    fn status_hostname_for_xip() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(127, 0, 0, 1)).unwrap();
        assert_eq!(
            routing.status_hostname().unwrap(),
            "127-0-0-1.xip.example.com"
        );
    }

    #[test]
    fn is_status_host_matches_for_xip_bare_domain() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(127, 0, 0, 1)).unwrap();
        assert!(routing.is_status_host("127-0-0-1.xip.example.com"));
        assert!(routing.is_status_host("192-0-2-10.xip.example.com"));
        assert!(routing.is_status_host("127-0-0-1.XIP.EXAMPLE.COM"));
        assert!(!routing.is_status_host("192-0-2-256.xip.example.com"));
    }

    #[test]
    fn is_status_host_does_not_match_service_hostname() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(127, 0, 0, 1)).unwrap();
        assert!(!routing.is_status_host("vite-127-0-0-1.xip.example.com"));
    }

    #[test]
    fn is_status_host_matches_bare_suffix() {
        let routing = HostRouting::Suffix(".localhost".to_string());
        assert!(routing.is_status_host("localhost"));
        assert!(routing.is_status_host("LOCALHOST"));
    }

    #[test]
    fn request_method_path_returns_method_and_path() {
        assert_eq!(
            request_method_path(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            Some(("GET", "/"))
        );
        assert_eq!(
            request_method_path(b"GET /api/status HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            Some(("GET", "/api/status"))
        );
        assert_eq!(
            request_method_path(b"POST / HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            Some(("POST", "/"))
        );
        assert_eq!(
            request_method_path(b"GET /vite HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            Some(("GET", "/vite"))
        );
        assert_eq!(request_method_path(b"not-http"), None);
    }

    #[tokio::test]
    async fn status_is_empty_when_no_services() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        assert_eq!(registry.status().await, "no services registered\n");
    }

    #[tokio::test]
    async fn status_json_is_empty_array_when_no_services() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        assert_eq!(registry.status_json().await, "[]");
    }

    #[tokio::test]
    async fn collect_rows_and_status_agree() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let (port, _) = free_port_range(1);
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (control, messages) =
            register_http(&registry, "vite", PortRequest::Fixed { port }).await;

        let rows = registry.collect_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "vite");
        assert_eq!(rows[0].kind, "http");
        assert_eq!(rows[0].state, "dormant");
        assert_eq!(rows[0].url, "http://vite.localhost:8080");
        assert_eq!(rows[0].upstream, "-");
        assert_eq!(rows[0].detail, "-");

        let text = registry.status().await;
        let expected = "NAME\tKIND\tSTATE\tURL\tUPSTREAM\tDETAIL\nvite\thttp\tdormant\thttp://vite.localhost:8080\t-\t-\n";
        assert_eq!(text, expected);

        drop(messages);
        drop(control);
    }

    #[tokio::test]
    async fn collect_rows_shows_ready_state_and_upstream() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let (port, _) = free_port_range(1);
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (control, messages) =
            register_http(&registry, "web", PortRequest::Fixed { port }).await;

        let ready = tokio::spawn(mark_ready(registry.clone(), "web", messages));
        registry.start("web").await.unwrap();
        let _port = ready.await.unwrap();

        let rows = registry.collect_rows().await;
        assert_eq!(rows[0].state, "ready");
        assert_eq!(rows[0].upstream, format!("127.0.0.1:{port}"));

        drop(control);
    }

    #[tokio::test]
    async fn status_json_contains_service_data() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let (port, _) = free_port_range(1);
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (control, messages) =
            register_http(&registry, "vite", PortRequest::Fixed { port }).await;

        let json = registry.status_json().await;
        assert!(json.contains("\"name\":\"vite\""));
        assert!(json.contains("\"kind\":\"http\""));
        assert!(json.contains("\"state\":\"dormant\""));
        assert!(json.contains("\"url\":\"http://vite.localhost:8080\""));

        drop(messages);
        drop(control);
    }

    #[tokio::test]
    async fn try_serve_status_serves_html_on_bare_suffix_root() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (mut client, mut server) = tokio::io::duplex(16 * 1024);

        let served = registry
            .try_serve_status(
                "localhost",
                b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(served);

        let mut buf = [0u8; 16 * 1024];
        let n = server.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("HTTP/1.1 200 OK"));
        assert!(
            !response.contains("&quot;"),
            "shell must not contain HTML entities in script: {response}"
        );
        assert!(
            response.contains("fetch(\"/api/status\")"),
            "shell must use normal JS quotes: {response}"
        );
        assert!(
            response.contains("setTimeout(poll, 2000)"),
            "shell must reschedule with 2000ms: {response}"
        );
        assert!(
            !response.contains("http-equiv=\"refresh\""),
            "shell must not have meta refresh: {response}"
        );
        assert!(
            !response.contains("innerHTML"),
            "shell must not use innerHTML: {response}"
        );
        assert!(response.contains("fetch(\"/api/stop\""));
        assert!(response.contains("\"X-Lazy-Service\": service.name"));
        assert!(response.contains("document.createElement(\"button\")"));
        assert!(response.contains("button.className = \"stop-link\""));
        assert!(response.contains("button.disabled = !canStop || stopping.has(service.name)"));
        assert!(response.contains("button.textContent = \"Stop\""));
        assert!(!response.contains("Stopping…"));
    }

    #[tokio::test]
    async fn try_serve_status_serves_json_on_bare_suffix_api() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (mut client, mut server) = tokio::io::duplex(4096);

        let served = registry
            .try_serve_status(
                "localhost",
                b"GET /api/status HTTP/1.1\r\nHost: localhost\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(served);

        let mut buf = [0u8; 4096];
        let n = server.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("HTTP/1.1 200 OK"));
        assert!(response.contains("application/json"));
    }

    #[tokio::test]
    async fn try_serve_status_serves_html_on_bare_xip_root() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(127, 0, 0, 1)).unwrap();
        let registry = registry(routing, 443, true);
        let (mut client, mut server) = tokio::io::duplex(16 * 1024);

        let served = registry
            .try_serve_status(
                "127-0-0-1.xip.example.com",
                b"GET / HTTP/1.1\r\nHost: 127-0-0-1.xip.example.com\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(served);

        let mut buf = [0u8; 16 * 1024];
        let n = server.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("HTTP/1.1 200 OK"));
    }

    #[tokio::test]
    async fn try_serve_status_serves_json_on_bare_xip_api() {
        let routing = HostRouting::xip("xip.example.com", Ipv4Addr::new(127, 0, 0, 1)).unwrap();
        let registry = registry(routing, 443, true);
        let (mut client, mut server) = tokio::io::duplex(4096);

        let served = registry
            .try_serve_status(
                "127-0-0-1.xip.example.com",
                b"GET /api/status HTTP/1.1\r\nHost: 127-0-0-1.xip.example.com\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(served);

        let mut buf = [0u8; 4096];
        let n = server.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("HTTP/1.1 200 OK"));
        assert!(response.contains("application/json"));
    }

    #[tokio::test]
    async fn try_serve_status_rejects_post_method() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (mut client, _server) = tokio::io::duplex(4096);

        let served = registry
            .try_serve_status(
                "localhost",
                b"POST / HTTP/1.1\r\nHost: localhost\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(!served, "POST / must not be served as status");

        let served = registry
            .try_serve_status(
                "localhost",
                b"POST /api/status HTTP/1.1\r\nHost: localhost\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(!served, "POST /api/status must not be served as status");
    }

    #[tokio::test]
    async fn try_serve_status_dispatches_stop_to_registered_service() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (control, mut messages) =
            register_http(&registry, "vite", PortRequest::Fixed { port: 4100 }).await;
        let (mut client, mut server) = tokio::io::duplex(4096);

        let served = registry
            .try_serve_status(
                "localhost",
                b"POST /api/stop HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nX-Lazy-Service: vite\r\nContent-Length: 2\r\n\r\n{}",
                &mut client,
            )
            .await
            .unwrap();
        assert!(served);
        assert!(matches!(messages.recv().await, Some(DaemonMessage::Stop)));

        let mut buf = [0u8; 4096];
        let n = server.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("HTTP/1.1 202 Accepted"));
        assert!(response.ends_with(r#"{"ok":true}"#));

        drop(control);
    }

    #[tokio::test]
    async fn try_serve_status_validates_stop_requests() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);

        for (request, expected_status) in [
            (
                b"POST /api/stop HTTP/1.1\r\nHost: localhost\r\nX-Lazy-Service: vite\r\n\r\n"
                    .as_slice(),
                "415 Unsupported Media Type",
            ),
            (
                b"POST /api/stop HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\r\n"
                    .as_slice(),
                "400 Bad Request",
            ),
            (
                b"POST /api/stop HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json; charset=utf-8\r\nX-Lazy-Service: missing\r\n\r\n"
                    .as_slice(),
                "404 Not Found",
            ),
        ] {
            let (mut client, mut server) = tokio::io::duplex(4096);
            assert!(
                registry
                    .try_serve_status("localhost", request, &mut client)
                    .await
                    .unwrap()
            );
            let mut buf = [0u8; 4096];
            let n = server.read(&mut buf).await.unwrap();
            let response = String::from_utf8_lossy(&buf[..n]);
            assert!(
                response.contains(expected_status),
                "expected {expected_status}, got {response}"
            );
        }
    }

    #[tokio::test]
    async fn try_serve_status_rejects_unrelated_path_on_status_host() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (mut client, _server) = tokio::io::duplex(4096);

        let served = registry
            .try_serve_status(
                "localhost",
                b"GET /some-other-path HTTP/1.1\r\nHost: localhost\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(!served);
    }

    #[tokio::test]
    async fn try_serve_status_does_not_match_service_host() {
        let registry = registry(HostRouting::Suffix(".localhost".to_string()), 8080, false);
        let (mut client, _server) = tokio::io::duplex(4096);

        let served = registry
            .try_serve_status(
                "vite.localhost",
                b"GET / HTTP/1.1\r\nHost: vite.localhost\r\n\r\n",
                &mut client,
            )
            .await
            .unwrap();
        assert!(!served);
    }
}
