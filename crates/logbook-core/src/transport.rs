//! The daemon's local socket: a Unix socket only the user can open.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::time::Duration;

pub type Stream = std::os::unix::net::UnixStream;

pub fn connect_to(path: &Path, timeout: Option<Duration>) -> Result<Stream> {
    let s = Stream::connect(path).with_context(|| format!("cannot reach {}", path.display()))?;
    s.set_read_timeout(timeout)?;
    s.set_write_timeout(timeout)?;
    Ok(s)
}

/// Connects to the Logbook daemon.
pub fn connect() -> Result<Stream> {
    let path = crate::paths::socket_path();
    connect_to(&path, None).map_err(|_| {
        anyhow!(
            "cannot reach the Logbook daemon at {} (start it with `logbook daemon`)",
            path.display()
        )
    })
}

pub fn daemon_alive() -> bool {
    connect().is_ok()
}

pub struct Listener {
    inner: std::os::unix::net::UnixListener,
    /// Held for the daemon's lifetime; the kernel drops the lock if it dies.
    _lock: std::fs::File,
}

impl Listener {
    pub fn bind() -> Result<Listener> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::paths::runtime_dir();
        std::fs::create_dir_all(&dir)?;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let path = crate::paths::socket_path();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(crate::paths::lock_path())?;
        // Two daemons started at once (the bar module and a key bind) must not
        // both bind: the second would unlink the first one's socket.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(anyhow!(
                "a Logbook daemon is already running ({})",
                path.display()
            ));
        }
        let _ = std::fs::remove_file(&path);
        let inner = std::os::unix::net::UnixListener::bind(&path)
            .with_context(|| format!("binding {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Listener { inner, _lock: lock })
    }

    pub fn accept(&self) -> std::io::Result<Stream> {
        self.inner.accept().map(|(s, _)| s)
    }
}
