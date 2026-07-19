use anyhow::{Context, Result, anyhow};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const STATE_DIR_MODE: u32 = 0o700;
const CONTROL_SOCKET_MODE: u32 = 0o600;

pub fn state_dir() -> Result<PathBuf> {
    let dir = dirs::home_dir()
        .context("could not locate home directory")?
        .join(".lazy");
    ensure_private_directory(&dir)?;
    Ok(dir)
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("lazy.sock"))
}

pub fn secure_control_socket(path: &Path) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(CONTROL_SOCKET_MODE))
        .with_context(|| format!("could not restrict control socket {}", path.display()))
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("could not create state directory {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect state directory {}", path.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(anyhow!(
            "state directory {} is not a directory",
            path.display()
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(STATE_DIR_MODE))
        .with_context(|| format!("could not restrict state directory {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn test_path(label: &str) -> PathBuf {
        let id = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("lazy-state-{label}-{}-{id}", std::process::id()))
    }

    #[test]
    fn state_directory_permissions_are_private() {
        let path = test_path("dir");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        ensure_private_directory(&path).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            STATE_DIR_MODE
        );
        std::fs::remove_dir(path).unwrap();
    }

    #[test]
    fn control_socket_permissions_are_private() {
        let path = test_path("socket");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

        secure_control_socket(&path).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            CONTROL_SOCKET_MODE
        );
        drop(listener);
        std::fs::remove_file(path).unwrap();
    }
}
