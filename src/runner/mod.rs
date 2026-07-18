use anyhow::{Context, Result, anyhow};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use tokio::{
    io::BufReader,
    net::UnixStream,
    time::{Duration, Instant, sleep},
};

use crate::{
    command::{self, ports},
    ipc::{self, DaemonMessage, ProcessKind, Register, RunnerMessage, SocketMessage},
    state,
};

pub struct HttpConfig {
    pub name: String,
    pub command: Vec<String>,
    pub daemon_timeout: Option<Duration>,
    pub upstream_port: Option<u16>,
    pub framework: Option<String>,
    pub cwd: Option<PathBuf>,
    pub port_range_start: u16,
    pub port_range_end: u16,
}

pub struct WorkerConfig {
    pub name: String,
    pub command: Vec<String>,
    pub daemon_timeout: Option<Duration>,
    pub active_while: Vec<String>,
}

struct RunningProcess {
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

type ProcessSlot = Arc<Mutex<Option<RunningProcess>>>;

pub async fn run_http(config: HttpConfig) -> Result<()> {
    let cwd = config
        .cwd
        .as_ref()
        .map(std::fs::canonicalize)
        .transpose()
        .context("could not resolve --cwd")?;
    let port = match config.upstream_port {
        Some(port) => port,
        None => ports::find_free_port(config.port_range_start, config.port_range_end).await?,
    };
    let url = register_loop(
        Register {
            name: config.name.clone(),
            kind: ProcessKind::Http,
            upstream_port: Some(port),
            active_while: Vec::new(),
        },
        config.daemon_timeout,
    )
    .await?;

    let public_url = url.unwrap_or_else(|| format!("http://{}", config.name));
    println!("lazy: {} registered at {}", config.name, public_url);
    println!("lazy: waiting for traffic");

    let prepared = command::prepare_http_command(
        config.command,
        port,
        &public_url,
        config.framework.as_deref(),
    );
    run_control_loop(config.name, prepared.argv, prepared.env, cwd, Some(port)).await
}

pub async fn run_worker(config: WorkerConfig) -> Result<()> {
    register_loop(
        Register {
            name: config.name.clone(),
            kind: ProcessKind::Worker,
            upstream_port: None,
            active_while: config.active_while,
        },
        config.daemon_timeout,
    )
    .await?;

    println!("lazy: {} worker registered", config.name);
    println!("lazy: waiting for activation");
    run_control_loop(config.name, config.command, Vec::new(), None, None).await
}

async fn register_loop(
    register: Register,
    daemon_timeout: Option<Duration>,
) -> Result<Option<String>> {
    let path = state::socket_path()?;
    let mut stream = connect_to_daemon(&path, daemon_timeout).await?;
    ipc::send_json(&mut stream, &SocketMessage::RunnerRegister { register }).await?;

    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let response = ipc::read_json::<DaemonMessage>(&mut reader)
        .await?
        .ok_or_else(|| anyhow!("daemon disconnected during registration"))?;
    let stream = write.reunite(reader.into_inner())?;

    match response {
        DaemonMessage::Registered { url } => {
            CONTROL_STREAM
                .set(Mutex::new(Some(stream)))
                .map_err(|_| anyhow!("control stream already initialized"))?;
            Ok(url)
        }
        DaemonMessage::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected daemon response: {other:?}")),
    }
}

async fn connect_to_daemon(path: &std::path::Path, wait: Option<Duration>) -> Result<UnixStream> {
    let deadline = wait.map(|duration| Instant::now() + duration);

    loop {
        match UnixStream::connect(path).await {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                let Some(deadline) = deadline else {
                    return Err(error).with_context(|| {
                        format!("could not connect to lazy daemon at {}", path.display())
                    });
                };

                let now = Instant::now();
                if now >= deadline {
                    return Err(anyhow!(
                        "timed out waiting for lazy daemon at {}: {error}",
                        path.display()
                    ));
                }
                sleep((deadline - now).min(Duration::from_millis(100))).await;
            }
        }
    }
}

static CONTROL_STREAM: std::sync::OnceLock<Mutex<Option<UnixStream>>> = std::sync::OnceLock::new();

async fn run_control_loop(
    name: String,
    argv: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
    readiness_port: Option<u16>,
) -> Result<()> {
    let stream = CONTROL_STREAM
        .get()
        .and_then(|slot| slot.lock().ok()?.take())
        .ok_or_else(|| anyhow!("control stream missing"))?;
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let process: ProcessSlot = Arc::new(Mutex::new(None));

    while let Some(message) = ipc::read_json::<DaemonMessage>(&mut reader).await? {
        match message {
            DaemonMessage::Start => {
                if process.lock().unwrap().is_some() {
                    ipc::send_json(&mut write, &RunnerMessage::Ready { name: name.clone() })
                        .await?;
                    continue;
                }

                println!("lazy: starting {}", name);
                match spawn_pty(argv.clone(), env.clone(), cwd.clone(), process.clone()) {
                    Ok(()) => {
                        if let Some(port) = readiness_port {
                            match ports::wait_for_port(port, Duration::from_secs(300)).await {
                                Ok(()) => {
                                    ipc::send_json(
                                        &mut write,
                                        &RunnerMessage::Ready { name: name.clone() },
                                    )
                                    .await?;
                                }
                                Err(err) => {
                                    stop_process(&process);
                                    ipc::send_json(
                                        &mut write,
                                        &RunnerMessage::Failed {
                                            name: name.clone(),
                                            error: err.to_string(),
                                        },
                                    )
                                    .await?;
                                }
                            }
                        } else {
                            ipc::send_json(
                                &mut write,
                                &RunnerMessage::Ready { name: name.clone() },
                            )
                            .await?;
                        }
                    }
                    Err(err) => {
                        ipc::send_json(
                            &mut write,
                            &RunnerMessage::Failed {
                                name: name.clone(),
                                error: err.to_string(),
                            },
                        )
                        .await?;
                    }
                }
            }
            DaemonMessage::Stop => {
                println!("lazy: stopping {}", name);
                stop_process(&process);
                ipc::send_json(&mut write, &RunnerMessage::Stopped { name: name.clone() }).await?;
                println!("lazy: waiting for activation");
            }
            DaemonMessage::Registered { .. } => {}
            DaemonMessage::Error { message } => eprintln!("lazy: daemon error: {message}"),
        }
    }

    Ok(())
}

fn spawn_pty(
    argv: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
    slot: ProcessSlot,
) -> Result<()> {
    if argv.is_empty() {
        return Err(anyhow!("empty command"));
    }

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 30,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new(&argv[0]);
    for arg in argv.iter().skip(1) {
        command.arg(arg);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    if let Some(cwd) = cwd {
        command.cwd(cwd);
    }

    let child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;

    thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let mut buf = [0; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = stdout.write_all(&buf[..n]);
                    let _ = stdout.flush();
                }
            }
        }
    });

    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = writer.write_all(&buf[..n]);
                    let _ = writer.flush();
                }
            }
        }
    });

    *slot.lock().unwrap() = Some(RunningProcess { child });
    Ok(())
}

fn stop_process(slot: &ProcessSlot) {
    if let Some(mut process) = slot.lock().unwrap().take() {
        let _ = process.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::net::UnixListener;

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

    fn test_socket_path(label: &str) -> PathBuf {
        let id = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("lazy-{label}-{}-{id}.sock", std::process::id()))
    }

    #[tokio::test]
    async fn connects_when_daemon_is_already_available() {
        let path = test_socket_path("ready");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            listener.accept().await.unwrap();
        });

        let stream = connect_to_daemon(&path, None).await.unwrap();
        drop(stream);
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn waits_for_delayed_daemon_availability() {
        let path = test_socket_path("delayed");
        let server_path = path.clone();
        let server = tokio::spawn(async move {
            sleep(Duration::from_millis(40)).await;
            let listener = UnixListener::bind(&server_path).unwrap();
            listener.accept().await.unwrap();
        });

        let stream = connect_to_daemon(&path, Some(Duration::from_secs(1)))
            .await
            .unwrap();
        drop(stream);
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn reports_timeout_when_daemon_stays_unavailable() {
        let path = test_socket_path("timeout");

        let error = connect_to_daemon(&path, Some(Duration::from_millis(25)))
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("timed out waiting for lazy daemon")
        );
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[tokio::test]
    async fn omitted_timeout_preserves_immediate_connection_error() {
        let path = test_socket_path("immediate");

        let error = connect_to_daemon(&path, None).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("could not connect to lazy daemon")
        );
        assert!(!error.to_string().contains("timed out"));
    }
}
