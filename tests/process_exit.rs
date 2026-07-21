use std::{
    fs::OpenOptions,
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

struct TestHome(PathBuf);

impl TestHome {
    fn new(label: &str) -> Self {
        let id = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("lazy-e2e-{label}-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn lazy(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lazy"));
    command.env("HOME", home);
    command
}

fn spawn_proxy(home: &Path) -> ChildGuard {
    let child = lazy(home)
        .args(["proxy", "--listen", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let socket = home.join(".lazy/lazy.sock");
    wait_until(Duration::from_secs(5), || socket.exists());
    ChildGuard(child)
}

fn spawn_http(home: &Path, name: &str, helper: &str, port: u16) -> ChildGuard {
    let child = lazy(home)
        .args([
            "http",
            name,
            "--port-range-start",
            &port.to_string(),
            "--port-range-end",
            &port.to_string(),
            "--",
            current_test_exe().to_str().unwrap(),
            "--exact",
            helper,
            "--nocapture",
        ])
        .env("LAZY_TEST_HELPER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_registration(home, name);
    ChildGuard(child)
}

fn spawn_worker(home: &Path, name: &str, helper: &str) -> ChildGuard {
    let child = lazy(home)
        .args([
            "worker",
            name,
            "--",
            current_test_exe().to_str().unwrap(),
            "--exact",
            helper,
            "--nocapture",
        ])
        .env("LAZY_TEST_HELPER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_registration(home, name);
    ChildGuard(child)
}

fn wait_for_registration(home: &Path, name: &str) {
    wait_until(Duration::from_secs(5), || status(home).contains(name));
}

fn start(home: &Path, name: &str) -> Output {
    lazy(home).args(["start", name]).output().unwrap()
}

fn status(home: &Path) -> String {
    let Ok(output) = lazy(home).arg("status").output() else {
        return String::new();
    };
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "condition timed out after {timeout:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn current_test_exe() -> PathBuf {
    std::env::current_exe().unwrap()
}

fn helper_enabled() -> bool {
    std::env::var_os("LAZY_TEST_HELPER").is_some()
}

#[test]
fn http_exit_before_readiness_fails_promptly_with_exit_context() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("http-before-ready");
    let _proxy = spawn_proxy(&home.0);
    let _runner = spawn_http(&home.0, "early", "helper_exit_immediately", free_port());

    let started = Instant::now();
    let output = start(&home.0, "early");
    let response = String::from_utf8_lossy(&output.stdout);

    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        response.contains("process exited with code 0"),
        "{response}"
    );
    let status = status(&home.0);
    assert!(status.contains("early\thttp\tfailed"), "{status}");
    assert!(
        status.contains("\t-\tprocess exited with code 0"),
        "{status}"
    );
}

#[test]
fn http_exit_after_readiness_releases_port_and_can_restart() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("http-after-ready");
    let counter = home.0.join("starts");
    let _proxy = spawn_proxy(&home.0);
    let port = free_port();
    let child = lazy(&home.0)
        .args([
            "http",
            "restarting",
            "--port-range-start",
            &port.to_string(),
            "--port-range-end",
            &port.to_string(),
            "--",
            current_test_exe().to_str().unwrap(),
            "--exact",
            "helper_http_then_exit",
            "--nocapture",
        ])
        .env("LAZY_TEST_HELPER", "1")
        .env("LAZY_TEST_COUNTER", &counter)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_registration(&home.0, "restarting");
    let _runner = ChildGuard(child);

    let first = String::from_utf8_lossy(&start(&home.0, "restarting").stdout).into_owned();
    assert!(first.contains("restarting: ready"), "{first}");
    wait_until(Duration::from_secs(5), || {
        status(&home.0).contains("restarting\thttp\tfailed")
    });
    let failed = status(&home.0);
    assert!(
        failed.contains("\t-\tprocess exited with code 0"),
        "{failed}"
    );

    let second = String::from_utf8_lossy(&start(&home.0, "restarting").stdout).into_owned();
    assert!(second.contains("restarting: ready"), "{second}");
    wait_until(Duration::from_secs(2), || count_starts(&counter) >= 2);
}

#[test]
fn worker_exit_is_reported_instead_of_ready() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("worker-exit");
    let _proxy = spawn_proxy(&home.0);
    let _runner = spawn_worker(&home.0, "jobs", "helper_exit_immediately");

    let response = String::from_utf8_lossy(&start(&home.0, "jobs").stdout).into_owned();
    assert!(
        response.contains("process exited with code 0"),
        "{response}"
    );
    let status = status(&home.0);
    assert!(status.contains("jobs\tworker\tfailed"), "{status}");
}

#[test]
fn worker_uses_runner_cwd_by_default_and_accepts_override() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("worker-cwd");
    let launch_dir = home.0.join("launch");
    let override_dir = home.0.join("override");
    std::fs::create_dir_all(&launch_dir).unwrap();
    std::fs::create_dir_all(&override_dir).unwrap();
    let _proxy = spawn_proxy(&home.0);

    for (name, cwd, expected) in [
        ("default-cwd", None, &launch_dir),
        ("override-cwd", Some(&override_dir), &override_dir),
    ] {
        let cwd_file = home.0.join(name);
        let mut command = lazy(&home.0);
        command.args(["worker", name]).current_dir(&launch_dir);
        if let Some(cwd) = cwd {
            command.args(["--cwd", cwd.to_str().unwrap()]);
        }
        let child = command
            .args([
                "--",
                current_test_exe().to_str().unwrap(),
                "--exact",
                "helper_write_cwd",
                "--nocapture",
            ])
            .env("LAZY_TEST_HELPER", "1")
            .env("LAZY_TEST_CWD_FILE", &cwd_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for_registration(&home.0, name);
        let _runner = ChildGuard(child);

        let _ = start(&home.0, name);
        wait_until(Duration::from_secs(2), || cwd_file.exists());
        assert_eq!(
            PathBuf::from(std::fs::read_to_string(&cwd_file).unwrap()),
            std::fs::canonicalize(expected).unwrap()
        );
    }
}

#[cfg(unix)]
#[test]
fn control_stream_loss_kills_and_reaps_active_child() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("disconnect");
    let pid_file = home.0.join("child-pid");
    let mut proxy = spawn_proxy(&home.0);
    let child = lazy(&home.0)
        .args([
            "worker",
            "long",
            "--",
            current_test_exe().to_str().unwrap(),
            "--exact",
            "helper_run_forever",
            "--nocapture",
        ])
        .env("LAZY_TEST_HELPER", "1")
        .env("LAZY_TEST_PID_FILE", &pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_registration(&home.0, "long");
    let mut runner = ChildGuard(child);
    let response = String::from_utf8_lossy(&start(&home.0, "long").stdout).into_owned();
    assert!(response.contains("long: ready"), "{response}");
    wait_until(Duration::from_secs(2), || pid_file.exists());
    let pid = std::fs::read_to_string(&pid_file).unwrap();

    proxy.0.kill().unwrap();
    proxy.0.wait().unwrap();
    wait_until(Duration::from_secs(5), || {
        runner.0.try_wait().unwrap().is_some()
    });
    wait_until(Duration::from_secs(5), || !process_exists(pid.trim()));
}

#[cfg(unix)]
#[test]
fn sigterm_removes_the_control_socket() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("sigterm");
    let mut proxy = spawn_proxy(&home.0);
    let socket = home.0.join(".lazy/lazy.sock");
    let pid_file = home.0.join("child-pid");
    let child = lazy(&home.0)
        .args([
            "worker",
            "sigterm-child",
            "--",
            current_test_exe().to_str().unwrap(),
            "--exact",
            "helper_run_forever",
            "--nocapture",
        ])
        .env("LAZY_TEST_HELPER", "1")
        .env("LAZY_TEST_PID_FILE", &pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_registration(&home.0, "sigterm-child");
    let mut runner = ChildGuard(child);
    let response = String::from_utf8_lossy(&start(&home.0, "sigterm-child").stdout).into_owned();
    assert!(response.contains("sigterm-child: ready"), "{response}");
    wait_until(Duration::from_secs(2), || pid_file.exists());
    let pid = std::fs::read_to_string(&pid_file).unwrap();

    let status = Command::new("kill")
        .args(["-TERM", &proxy.0.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(proxy.0.wait().unwrap().success());
    assert!(!socket.exists());
    wait_until(Duration::from_secs(5), || {
        runner.0.try_wait().unwrap().is_some()
    });
    wait_until(Duration::from_secs(5), || !process_exists(pid.trim()));
}

fn count_starts(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|contents| contents.lines().count())
        .unwrap_or(0)
}

#[cfg(unix)]
fn process_exists(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn spawn_proxy_on(home: &Path, extra_args: &[&str]) -> (ChildGuard, u16) {
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    let mut args = vec!["proxy", "--listen", &listen];
    args.extend_from_slice(extra_args);
    let child = lazy(home)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let socket = home.join(".lazy/lazy.sock");
    wait_until(Duration::from_secs(5), || socket.exists());
    (ChildGuard(child), port)
}

fn http_get(host: &str, port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("could not connect to proxy");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn parse_http_response(raw: &str) -> (u16, String) {
    let mut lines = raw.lines();
    let status_line = lines.next().unwrap_or("");
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let body_start = raw.find("\r\n\r\n").map(|p| p + 4).unwrap_or(raw.len());
    (code, raw[body_start..].to_string())
}

#[test]
fn suffix_status_root_serves_html_shell() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("suffix-html");
    let (_proxy, port) = spawn_proxy_on(&home.0, &[]);
    let raw = http_get("localhost", port, "/");
    let (code, body) = parse_http_response(&raw);
    assert_eq!(code, 200, "expected 200, got response: {raw}");
    assert!(body.contains("loading"));
    assert!(body.contains("fetch"));
    assert!(body.contains("/api/status"));
}

#[test]
fn suffix_status_api_returns_json() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("suffix-json");
    let (_proxy, port) = spawn_proxy_on(&home.0, &[]);
    let raw = http_get("localhost", port, "/api/status");
    let (code, body) = parse_http_response(&raw);
    assert_eq!(code, 200, "expected 200, got response: {raw}");
    assert!(body.starts_with('['), "expected JSON array, got: {raw}");
}

#[test]
fn suffix_status_unrelated_path_returns_not_found() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("suffix-404");
    let (_proxy, port) = spawn_proxy_on(&home.0, &[]);
    let raw = http_get("localhost", port, "/some-other-path");
    let (code, _) = parse_http_response(&raw);
    assert_ne!(code, 200, "unrelated path should not return 200: {raw}");
}

#[test]
fn suffix_service_host_routes_to_service_not_status() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("suffix-service");
    let (_proxy, port) = spawn_proxy_on(&home.0, &[]);
    // Service hosts like demo.localhost must NOT be interpreted as status
    let raw = http_get("demo.localhost", port, "/");
    let (code, _) = parse_http_response(&raw);
    assert_ne!(code, 200, "service host should not get status page: {raw}");
    // Service hosts get routed to the proxy which eventually fails (no such service)
}

#[test]
fn xip_status_root_serves_html_shell() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("xip-html");
    let (_proxy, port) = spawn_proxy_on(
        &home.0,
        &["--xip-domain", "xip.example.com", "--xip-ip", "127.0.0.1"],
    );
    let raw = http_get("127-0-0-1.xip.example.com", port, "/");
    let (code, body) = parse_http_response(&raw);
    assert_eq!(code, 200, "expected 200, got response: {raw}");
    assert!(body.contains("loading"));
    assert!(body.contains("fetch"));
    assert!(body.contains("/api/status"));
}

#[test]
fn xip_status_api_returns_json() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("xip-json");
    let (_proxy, port) = spawn_proxy_on(
        &home.0,
        &["--xip-domain", "xip.example.com", "--xip-ip", "127.0.0.1"],
    );
    let raw = http_get("127-0-0-1.xip.example.com", port, "/api/status");
    let (code, body) = parse_http_response(&raw);
    assert_eq!(code, 200, "expected 200, got response: {raw}");
    assert!(body.starts_with('['), "expected JSON array, got: {raw}");
}

#[test]
fn xip_status_unrelated_path_returns_not_found() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("xip-404");
    let (_proxy, port) = spawn_proxy_on(
        &home.0,
        &["--xip-domain", "xip.example.com", "--xip-ip", "127.0.0.1"],
    );
    let raw = http_get("127-0-0-1.xip.example.com", port, "/some-other-path");
    let (code, _) = parse_http_response(&raw);
    assert_ne!(code, 200, "unrelated path should not return 200: {raw}");
}

#[test]
fn xip_service_host_routes_to_service_not_status() {
    if helper_enabled() {
        return;
    }
    let home = TestHome::new("xip-service");
    let (_proxy, port) = spawn_proxy_on(
        &home.0,
        &["--xip-domain", "xip.example.com", "--xip-ip", "127.0.0.1"],
    );
    // Service hostnames include the encoded IP as a suffix, so they don't match the bare host
    let raw = http_get("demo-127-0-0-1.xip.example.com", port, "/");
    let (code, _) = parse_http_response(&raw);
    assert_ne!(code, 200, "service host should not get status page: {raw}");
}

#[test]
fn helper_exit_immediately() {
    // The integration test executable doubles as a dependency-free child fixture.
}

#[test]
fn helper_http_then_exit() {
    if !helper_enabled() {
        return;
    }
    if let Some(path) = std::env::var_os("LAZY_TEST_COUNTER") {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "start").unwrap();
    }
    let port: u16 = std::env::var("PORT").unwrap().parse().unwrap();
    let _listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    thread::sleep(Duration::from_millis(400));
}

#[test]
fn helper_run_forever() {
    if !helper_enabled() {
        return;
    }
    std::fs::write(
        std::env::var_os("LAZY_TEST_PID_FILE").unwrap(),
        std::process::id().to_string(),
    )
    .unwrap();
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn helper_write_cwd() {
    if !helper_enabled() {
        return;
    }
    let cwd = std::env::current_dir().unwrap();
    std::fs::write(
        std::env::var_os("LAZY_TEST_CWD_FILE").unwrap(),
        cwd.to_string_lossy().as_bytes(),
    )
    .unwrap();
}
