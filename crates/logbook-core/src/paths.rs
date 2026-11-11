//! Where Logbook keeps things. Every location can be moved with an
//! environment variable, which tests and development builds use to stay away
//! from an installed copy.

use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Expands a leading `~`.
pub fn expand(s: &str) -> PathBuf {
    let s = s.trim();
    if s == "~" {
        home()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(s)
    }
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn xdg_config() -> PathBuf {
    env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

pub fn config_dir() -> PathBuf {
    env_dir("LOGBOOK_CONFIG_DIR").unwrap_or_else(|| xdg_config().join("logbook"))
}

/// Logs and the transcription queue, which must survive a reboot.
pub fn state_dir() -> PathBuf {
    if let Some(d) = env_dir("LOGBOOK_STATE_DIR") {
        return d;
    }
    env_dir("XDG_STATE_HOME")
        .unwrap_or_else(|| home().join(".local/state"))
        .join("logbook")
}

/// The user's runtime directory, which logout and reboot clear.
pub fn xdg_runtime() -> Option<PathBuf> {
    env_dir("XDG_RUNTIME_DIR")
}

/// The socket, the daemon lock and the live recording record. It lives in
/// `$XDG_RUNTIME_DIR`, never `/tmp`: on a machine where `/tmp` is on disk a
/// crash would leave a record that outlives the reboot and lights the lamp
/// for a recording that is not there.
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = env_dir("LOGBOOK_RUNTIME_DIR") {
        return d;
    }
    if let Some(d) = xdg_runtime() {
        return d.join("logbook");
    }
    let base = env_dir("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("logbook-{}", unsafe { libc::getuid() }))
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("logbook.sock")
}

/// Held (flock) by the running daemon, so two daemons started at once by the
/// bar module and a key bind cannot both bind the socket.
pub fn lock_path() -> PathBuf {
    runtime_dir().join("daemon.lock")
}

/// What is recording right now, and the process groups that do it. A daemon
/// that starts and finds this file cleans up after one that crashed.
pub fn live_file() -> PathBuf {
    runtime_dir().join("live.json")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// The user's stylesheet for the drawer, loaded over the theme.
pub fn style_file() -> PathBuf {
    config_dir().join("style.css")
}

/// Sessions waiting to be transcribed.
pub fn queue_file() -> PathBuf {
    state_dir().join("queue.json")
}

pub fn board_log() -> PathBuf {
    state_dir().join("board.log")
}

pub fn daemon_log() -> PathBuf {
    state_dir().join("daemon.log")
}

/// Waybar's config directory.
pub fn waybar_dir() -> PathBuf {
    xdg_config().join("waybar")
}

/// voxtype's runtime directory: its `state` and `pid` files.
pub fn voxtype_dir() -> Option<PathBuf> {
    if let Some(d) = env_dir("LOGBOOK_VOXTYPE_DIR") {
        return Some(d);
    }
    xdg_runtime().map(|d| d.join("voxtype"))
}

/// Switchyard's socket, as Switchyard itself finds it.
pub fn switchyard_socket() -> PathBuf {
    if let Some(d) = env_dir("SWITCHYARD_RUNTIME_DIR") {
        return d.join("yard.sock");
    }
    if let Some(d) = xdg_runtime() {
        return d.join("switchyard").join("yard.sock");
    }
    let base = env_dir("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("switchyard-{}", unsafe { libc::getuid() }))
        .join("yard.sock")
}

/// Switchyard's task overlays, where `logbook setup` writes its task.
pub fn switchyard_tasks_dir() -> PathBuf {
    env_dir("SWITCHYARD_CONFIG_DIR")
        .unwrap_or_else(|| xdg_config().join("switchyard"))
        .join("tasks.d")
}

/// Shows a path under the home directory as `~/...`.
pub fn tilde(p: &Path) -> String {
    let home = home();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// Finds a program on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = expand(name);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}
