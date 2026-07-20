use anyhow::{Context, Result, anyhow};
use std::{
    io::ErrorKind,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::OnceLock,
};

const STATE_DIR_MODE: u32 = 0o700;
const CONTROL_SOCKET_MODE: u32 = 0o600;
static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_state_dir(path: PathBuf) -> Result<()> {
    STATE_DIR
        .set(path)
        .map_err(|_| anyhow!("state directory was already configured"))
}

pub fn state_dir() -> Result<PathBuf> {
    let dir = match STATE_DIR.get() {
        Some(path) => path.clone(),
        None => dirs::home_dir()
            .context("could not locate home directory")?
            .join(".lazy"),
    };
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

pub fn remove_stale_control_socket(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_socket() {
        return Err(anyhow!(
            "refusing to remove non-socket control path {}",
            path.display()
        ));
    }
    ensure_current_user_owns(path, &metadata)?;

    match UnixStream::connect(path) {
        Ok(_) => Err(anyhow!(
            "another lazy daemon is already listening at {}",
            path.display()
        )),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::ConnectionRefused | ErrorKind::NotFound
            ) =>
        {
            remove_if_unchanged(path, &metadata)
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "could not inspect existing control socket {}",
                path.display()
            )
        }),
    }
}

fn remove_if_unchanged(path: &Path, original: &std::fs::Metadata) -> Result<()> {
    let current = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if current.dev() != original.dev() || current.ino() != original.ino() {
        return Err(anyhow!(
            "control socket {} changed while checking whether it was stale",
            path.display()
        ));
    }
    std::fs::remove_file(path)
        .with_context(|| format!("could not remove stale control socket {}", path.display()))
}

pub struct ControlSocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl ControlSocketGuard {
    pub fn new(path: PathBuf) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("could not inspect control socket {}", path.display()))?;
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl Drop for ControlSocketGuard {
    fn drop(&mut self) {
        let Ok(metadata) = std::fs::symlink_metadata(&self.path) else {
            return;
        };
        if metadata.file_type().is_socket()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
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
    ensure_current_user_owns(path, &metadata)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(STATE_DIR_MODE))
        .with_context(|| format!("could not restrict state directory {}", path.display()))
}

fn ensure_current_user_owns(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(anyhow!(
            "{} is owned by UID {}, but lazy is running as UID {}",
            path.display(),
            metadata.uid(),
            effective_uid
        ));
    }
    Ok(())
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

    #[test]
    fn removes_an_owned_stale_control_socket() {
        let path = test_path("stale-socket");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(listener);

        remove_stale_control_socket(&path).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn refuses_to_remove_a_live_control_socket() {
        let path = test_path("live-socket");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

        let error = remove_stale_control_socket(&path).unwrap_err();

        assert!(error.to_string().contains("already listening"));
        drop(listener);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn refuses_to_remove_a_non_socket_control_path() {
        let path = test_path("regular-file");
        std::fs::write(&path, "not a socket").unwrap();

        let error = remove_stale_control_socket(&path).unwrap_err();

        assert!(error.to_string().contains("non-socket"));
        std::fs::remove_file(path).unwrap();
    }
}
