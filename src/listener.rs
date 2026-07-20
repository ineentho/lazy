use anyhow::{Context, Result, anyhow};
use std::net::{SocketAddr, TcpListener};

#[derive(Debug, Clone)]
pub enum Source {
    Bind(SocketAddr),
    Activated(String),
}

pub fn acquire(source: &Source) -> Result<TcpListener> {
    let listener = match source {
        Source::Bind(address) => TcpListener::bind(address)
            .with_context(|| format!("could not bind proxy listener {address}"))?,
        Source::Activated(name) => activated(name)?,
    };
    listener
        .set_nonblocking(true)
        .context("could not make proxy listener nonblocking")?;
    set_close_on_exec(&listener)?;
    Ok(listener)
}

fn set_close_on_exec(listener: &TcpListener) -> Result<()> {
    use std::os::fd::AsRawFd;

    let fd = listener.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err(std::io::Error::last_os_error())
            .context("could not set close-on-exec on listener");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn activated(name: &str) -> Result<TcpListener> {
    let pid = environment_number("LISTEN_PID")?;
    let count = environment_number("LISTEN_FDS")? as usize;
    let names = std::env::var("LISTEN_FDNAMES")
        .context("LISTEN_FDNAMES is required for named socket activation")?;
    let fd = select_systemd_fd(name, std::process::id(), pid, count, &names)?;
    adopt_listener_fd(fd)
}

#[cfg(any(target_os = "linux", test))]
fn select_systemd_fd(
    name: &str,
    expected_pid: u32,
    pid: u64,
    count: usize,
    names: &str,
) -> Result<libc::c_int> {
    if pid != u64::from(expected_pid) {
        return Err(anyhow!("LISTEN_PID is {pid}, expected {expected_pid}"));
    }
    let names: Vec<_> = names.split(':').collect();
    if names.len() != count {
        return Err(anyhow!(
            "LISTEN_FDNAMES contains {} names for {count} descriptors",
            names.len()
        ));
    }
    let matches: Vec<_> = names
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (*candidate == name).then_some(3 + index))
        .collect();
    let [fd] = matches.as_slice() else {
        return Err(anyhow!(
            "expected exactly one activated socket named {name:?}, found {}",
            matches.len()
        ));
    };
    i32::try_from(*fd).context("activated descriptor number is too large")
}

#[cfg(target_os = "linux")]
fn environment_number(name: &str) -> Result<u64> {
    std::env::var(name)
        .with_context(|| format!("{name} is not set; lazy was not started by systemd"))?
        .parse()
        .with_context(|| format!("{name} is not a valid number"))
}

#[cfg(target_os = "macos")]
fn activated(name: &str) -> Result<TcpListener> {
    use std::{ffi::CString, ptr};

    unsafe extern "C" {
        fn launch_activate_socket(
            name: *const libc::c_char,
            fds: *mut *mut libc::c_int,
            count: *mut libc::size_t,
        ) -> libc::c_int;
    }

    let name = CString::new(name).context("activated socket name contains a null byte")?;
    let mut fds = ptr::null_mut();
    let mut count = 0;
    let error = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut count) };
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error))
            .context("launchd did not provide the requested socket");
    }
    if count != 1 {
        for index in 0..count {
            unsafe { libc::close(*fds.add(index)) };
        }
        unsafe { libc::free(fds.cast()) };
        return Err(anyhow!(
            "expected exactly one launchd socket, found {count}"
        ));
    }
    let fd = unsafe { *fds };
    unsafe { libc::free(fds.cast()) };
    adopt_listener_fd(fd)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn activated(_name: &str) -> Result<TcpListener> {
    Err(anyhow!(
        "socket activation is only supported on Linux and macOS"
    ))
}

fn validate_listener_fd(fd: libc::c_int) -> Result<()> {
    let mut socket_type: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&socket_type) as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut socket_type as *mut libc::c_int).cast(),
            &mut length,
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error())
            .context("activated descriptor is not a socket");
    }
    if socket_type != libc::SOCK_STREAM {
        return Err(anyhow!("activated descriptor is not a TCP stream socket"));
    }

    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut address_length = std::mem::size_of_val(&address) as libc::socklen_t;
    if unsafe {
        libc::getsockname(
            fd,
            (&mut address as *mut libc::sockaddr_storage).cast(),
            &mut address_length,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error())
            .context("could not inspect activated socket address");
    }
    if !matches!(
        address.ss_family as libc::c_int,
        libc::AF_INET | libc::AF_INET6
    ) {
        return Err(anyhow!("activated descriptor is not a TCP/IP socket"));
    }

    validate_accepting(fd)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_accepting(fd: libc::c_int) -> Result<()> {
    let mut accepting: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&accepting) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ACCEPTCONN,
            (&mut accepting as *mut libc::c_int).cast(),
            &mut length,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error())
            .context("could not inspect activated listener state");
    }
    if accepting != 1 {
        return Err(anyhow!("activated descriptor is not a listening socket"));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_accepting(_fd: libc::c_int) -> Result<()> {
    // launch_activate_socket only returns descriptors created from a launchd
    // Sockets entry. macOS does not expose SO_ACCEPTCONN for an extra check.
    Ok(())
}

fn adopt_listener_fd(fd: libc::c_int) -> Result<TcpListener> {
    use std::os::fd::FromRawFd;

    if let Err(error) = validate_listener_fd(fd) {
        unsafe { libc::close(fd) };
        return Err(error);
    }
    Ok(unsafe { TcpListener::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_listener_is_nonblocking_and_close_on_exec() {
        use std::os::fd::AsRawFd;

        let listener = acquire(&Source::Bind("127.0.0.1:0".parse().unwrap())).unwrap();
        let descriptor_flags = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFD) };
        let status_flags = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFL) };

        assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
        assert_ne!(status_flags & libc::O_NONBLOCK, 0);
    }

    #[test]
    fn rejects_a_descriptor_that_is_not_a_tcp_stream() {
        use std::os::fd::AsRawFd;

        let stream = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(validate_listener_fd(stream.as_raw_fd()).is_err());
    }

    #[test]
    fn adopts_an_inherited_listener_descriptor() {
        use std::os::fd::AsRawFd;

        let original = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = original.local_addr().unwrap();
        let inherited_fd = unsafe { libc::dup(original.as_raw_fd()) };
        assert!(inherited_fd >= 0);

        let inherited = adopt_listener_fd(inherited_fd).unwrap();

        assert_eq!(inherited.local_addr().unwrap(), address);
    }

    #[test]
    fn selects_a_named_systemd_descriptor() {
        assert_eq!(
            select_systemd_fd("HTTPS", 42, 42, 2, "HTTP:HTTPS").unwrap(),
            4
        );
    }

    #[test]
    fn rejects_systemd_activation_for_another_process() {
        let error = select_systemd_fd("HTTPS", 42, 7, 1, "HTTPS").unwrap_err();
        assert!(error.to_string().contains("LISTEN_PID"));
    }

    #[test]
    fn rejects_ambiguous_systemd_descriptor_names() {
        let error = select_systemd_fd("HTTPS", 42, 42, 2, "HTTPS:HTTPS").unwrap_err();
        assert!(error.to_string().contains("found 2"));
    }
}
