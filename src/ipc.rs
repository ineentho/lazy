use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use crate::state;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerMessage {
    Register(Register),
    Ready { name: String },
    Stopped { name: String },
    Failed { name: String, error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    pub name: String,
    pub kind: ProcessKind,
    pub upstream_port: Option<u16>,
    pub active_while: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    Http,
    Worker,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMessage {
    Registered { url: Option<String> },
    Start,
    Stop,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    Status,
    Start { name: String },
    Stop { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SocketMessage {
    RunnerRegister { register: Register },
    Client { request: ClientRequest },
}

pub async fn send_json<W, T>(stream: &mut W, value: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(value)?;
    stream.write_all(&bytes).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

pub async fn read_json<T: for<'de> Deserialize<'de>>(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<Option<T>> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

pub async fn request(request: ClientRequest) -> Result<String> {
    let path = state::socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("could not connect to lazy daemon at {}", path.display()))?;
    send_json(&mut stream, &SocketMessage::Client { request }).await?;

    let (read, _) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut response = String::new();
    reader.read_to_string(&mut response).await?;
    Ok(response.trim_end().to_string())
}
