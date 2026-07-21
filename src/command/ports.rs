use anyhow::{Result, anyhow};
use std::net::{Ipv4Addr, Ipv6Addr};
use tokio::net::TcpStream;
use tokio::time::{Duration, Instant, sleep};

pub async fn connect_loopback(port: u16) -> std::io::Result<TcpStream> {
    match TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
        Ok(stream) => Ok(stream),
        Err(_) => TcpStream::connect((Ipv6Addr::LOCALHOST, port)).await,
    }
}

pub async fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if connect_loopback(port).await.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("timed out waiting for loopback port {port}"));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn connects_to_ipv6_loopback_when_ipv4_is_unavailable() {
        let listener = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();

        connect_loopback(port).await.unwrap();
    }
}
