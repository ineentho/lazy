use anyhow::{Context, Result, anyhow};
use portable_pty::{ChildKiller, CommandBuilder, ExitStatus, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
    thread,
};
use tokio::{
    io::BufReader,
    net::UnixStream,
    task::JoinHandle,
    time::{Duration, Instant, sleep},
};

use crate::{
    command::{self, ports},
    ipc::{self, DaemonMessage, PortRequest, ProcessKind, Register, RunnerMessage, SocketMessage},
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
    pub cwd: Option<PathBuf>,
}

struct RunningProcess {
    killer: Box<dyn ChildKiller + Send + Sync>,
    wait: JoinHandle<std::io::Result<ExitStatus>>,
}

type ReadinessCheck = JoinHandle<Result<()>>;

const WORKER_STARTUP_GRACE: Duration = Duration::from_millis(100);

pub async fn run_http(config: HttpConfig) -> Result<()> {
    let cwd = resolve_cwd(config.cwd.as_ref())?;
    let port_request = match config.upstream_port {
        Some(port) => PortRequest::Fixed { port },
        None => PortRequest::Range {
            start: config.port_range_start,
            end: config.port_range_end,
        },
    };
    let url = register_loop(
        Register {
            name: config.name.clone(),
            kind: ProcessKind::Http,
            port_request: Some(port_request),
        },
        config.daemon_timeout,
    )
    .await?;

    let public_url = url.unwrap_or_else(|| format!("http://{}", config.name));
    println!("lazy: {} registered at {}", config.name, public_url);
    println!("lazy: waiting for traffic");

    run_control_loop(
        config.name,
        config.command,
        cwd,
        Some(HttpCommand {
            public_url,
            framework: config.framework,
        }),
    )
    .await
}

pub async fn run_worker(config: WorkerConfig) -> Result<()> {
    let cwd = resolve_cwd(config.cwd.as_ref())?;
    register_loop(
        Register {
            name: config.name.clone(),
            kind: ProcessKind::Worker,
            port_request: None,
        },
        config.daemon_timeout,
    )
    .await?;

    println!("lazy: {} worker registered", config.name);
    println!("lazy: waiting for activation");
    run_control_loop(config.name, config.command, cwd, None).await
}

fn resolve_cwd(cwd: Option<&PathBuf>) -> Result<PathBuf> {
    match cwd {
        Some(cwd) => std::fs::canonicalize(cwd).context("could not resolve --cwd"),
        None => std::env::current_dir().context("could not resolve current working directory"),
    }
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
    cwd: PathBuf,
    http: Option<HttpCommand>,
) -> Result<()> {
    let stream = CONTROL_STREAM
        .get()
        .and_then(|slot| slot.lock().ok()?.take())
        .ok_or_else(|| anyhow!("control stream missing"))?;
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut process = None;
    let mut readiness = None;

    let result = control_loop(
        &name,
        &argv,
        cwd,
        http.as_ref(),
        &mut reader,
        &mut write,
        &mut process,
        &mut readiness,
    )
    .await;

    if let Some(check) = readiness.take() {
        check.abort();
    }
    stop_process(&mut process).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn control_loop(
    name: &str,
    argv: &[String],
    cwd: PathBuf,
    http: Option<&HttpCommand>,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    write: &mut tokio::net::unix::OwnedWriteHalf,
    process: &mut Option<RunningProcess>,
    readiness: &mut Option<ReadinessCheck>,
) -> Result<()> {
    loop {
        enum Event {
            Control(Result<Option<DaemonMessage>>),
            Exited(Result<std::io::Result<ExitStatus>, tokio::task::JoinError>),
            Readiness(Result<Result<()>, tokio::task::JoinError>),
        }

        let event = tokio::select! {
            message = ipc::read_json::<DaemonMessage>(reader) => Event::Control(message),
            result = async { (&mut process.as_mut().unwrap().wait).await }, if process.is_some() => {
                Event::Exited(result)
            }
            result = async { readiness.as_mut().unwrap().await }, if readiness.is_some() => {
                Event::Readiness(result)
            }
        };

        match event {
            Event::Control(Ok(Some(message))) => match message {
                DaemonMessage::Start { port } => {
                    if process.is_some() {
                        if readiness.is_none() {
                            ipc::send_json(
                                write,
                                &RunnerMessage::Ready {
                                    name: name.to_string(),
                                },
                            )
                            .await?;
                        }
                        continue;
                    }

                    println!("lazy: starting {}", name);
                    let prepared = match prepare_start_command(argv, http, port) {
                        Ok(prepared) => prepared,
                        Err(err) => {
                            ipc::send_json(
                                write,
                                &RunnerMessage::Failed {
                                    name: name.to_string(),
                                    error: err.to_string(),
                                },
                            )
                            .await?;
                            continue;
                        }
                    };
                    match spawn_pty(prepared.argv, prepared.env, cwd.clone()) {
                        Ok(running) => {
                            *process = Some(running);
                            *readiness = Some(tokio::spawn(async move {
                                match port {
                                    Some(port) => {
                                        ports::wait_for_port(port, Duration::from_secs(300)).await
                                    }
                                    None => {
                                        sleep(WORKER_STARTUP_GRACE).await;
                                        Ok(())
                                    }
                                }
                            }));
                        }
                        Err(err) => {
                            ipc::send_json(
                                write,
                                &RunnerMessage::Failed {
                                    name: name.to_string(),
                                    error: err.to_string(),
                                },
                            )
                            .await?;
                        }
                    }
                }
                DaemonMessage::Stop => {
                    println!("lazy: stopping {}", name);
                    if let Some(check) = readiness.take() {
                        check.abort();
                    }
                    stop_process(process).await;
                    ipc::send_json(
                        write,
                        &RunnerMessage::Stopped {
                            name: name.to_string(),
                        },
                    )
                    .await?;
                    println!("lazy: waiting for activation");
                }
                DaemonMessage::Registered { .. } => {}
                DaemonMessage::Error { message } => eprintln!("lazy: daemon error: {message}"),
            },
            Event::Control(Ok(None)) => return Ok(()),
            Event::Control(Err(error)) => return Err(error),
            Event::Exited(result) => {
                let _ = process.take();
                if let Some(check) = readiness.take() {
                    check.abort();
                }
                let error = describe_exit(result);
                eprintln!("lazy: {name} {error}");
                ipc::send_json(
                    write,
                    &RunnerMessage::Failed {
                        name: name.to_string(),
                        error,
                    },
                )
                .await?;
                println!("lazy: waiting for activation");
            }
            Event::Readiness(result) => {
                let _ = readiness.take();
                match result {
                    Ok(Ok(())) => {
                        ipc::send_json(
                            write,
                            &RunnerMessage::Ready {
                                name: name.to_string(),
                            },
                        )
                        .await?;
                    }
                    Ok(Err(error)) => {
                        stop_process(process).await;
                        ipc::send_json(
                            write,
                            &RunnerMessage::Failed {
                                name: name.to_string(),
                                error: error.to_string(),
                            },
                        )
                        .await?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
}

struct HttpCommand {
    public_url: String,
    framework: Option<String>,
}

fn prepare_start_command(
    argv: &[String],
    http: Option<&HttpCommand>,
    port: Option<u16>,
) -> Result<command::PreparedCommand> {
    match (http, port) {
        (Some(http), Some(port)) => Ok(command::prepare_http_command(
            argv.to_vec(),
            port,
            &http.public_url,
            http.framework.as_deref(),
        )),
        (None, None) => Ok(command::PreparedCommand {
            argv: argv.to_vec(),
            env: Vec::new(),
        }),
        (Some(_), None) => Err(anyhow!("HTTP service start did not include a port")),
        (None, Some(_)) => Err(anyhow!("worker start unexpectedly included a port")),
    }
}

fn spawn_pty(
    argv: Vec<String>,
    env: Vec<(String, String)>,
    cwd: PathBuf,
) -> Result<RunningProcess> {
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
    command.cwd(cwd);

    let mut child = pair.slave.spawn_command(command)?;
    let killer = child.clone_killer();
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

    let wait = tokio::task::spawn_blocking(move || child.wait());
    Ok(RunningProcess { killer, wait })
}

async fn stop_process(slot: &mut Option<RunningProcess>) {
    if let Some(mut process) = slot.take() {
        let _ = process.killer.kill();
        let _ = process.wait.await;
    }
}

fn describe_exit(result: Result<std::io::Result<ExitStatus>, tokio::task::JoinError>) -> String {
    match result {
        Ok(Ok(status)) => match status.signal() {
            Some(signal) => format!("process exited due to signal {signal}"),
            None => format!("process exited with code {}", status.exit_code()),
        },
        Ok(Err(error)) => format!("could not wait for process: {error}"),
        Err(error) => format!("process monitor failed: {error}"),
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

    #[test]
    fn prepares_http_command_with_port_received_at_start() {
        let argv = vec!["vite".to_string(), "dev".to_string()];
        let http = HttpCommand {
            public_url: "http://vite.localhost:8080".to_string(),
            framework: None,
        };

        let prepared = prepare_start_command(&argv, Some(&http), Some(4321)).unwrap();

        assert!(
            prepared
                .argv
                .windows(2)
                .any(|args| args == ["--port", "4321"])
        );
        assert!(
            prepared
                .env
                .contains(&("PORT".to_string(), "4321".to_string()))
        );
    }

    #[test]
    fn omitted_cwd_uses_runner_working_directory() {
        assert_eq!(resolve_cwd(None).unwrap(), std::env::current_dir().unwrap());
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
