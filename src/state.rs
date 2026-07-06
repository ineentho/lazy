use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn state_dir() -> Result<PathBuf> {
    let dir = dirs::home_dir()
        .context("could not locate home directory")?
        .join(".lazy");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("lazy.sock"))
}
