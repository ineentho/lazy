use anyhow::{Result, anyhow};
use tokio::net::TcpStream;
use tokio::time::{Duration, Instant, sleep};

pub async fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("timed out waiting for 127.0.0.1:{port}"));
        }
        sleep(Duration::from_millis(100)).await;
    }
}
