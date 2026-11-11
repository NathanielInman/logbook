//! voxtype's daemon state, from the files it keeps in
//! `$XDG_RUNTIME_DIR/voxtype`: `state` holds one word (`idle`, `recording`,
//! `transcribing`) and `pid` the daemon's process. The daemon watches the
//! folder with inotify, so dictation needs no `voxtype status` child.

use crate::model::Dictation;
use std::path::Path;

/// The state word voxtype wrote.
pub fn parse(state: &str) -> Dictation {
    match state.trim().to_ascii_lowercase().as_str() {
        "idle" | "ready" => Dictation::Idle,
        "recording" | "listening" => Dictation::Recording,
        "transcribing" | "processing" | "outputting" | "typing" => Dictation::Transcribing,
        _ => Dictation::Stopped,
    }
}

/// Whether a process is alive.
pub fn alive(pid: i32) -> bool {
    pid > 0
        && (unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// voxtype's pid, when its pid file names a live process.
pub fn pid(dir: &Path) -> Option<i32> {
    let p: i32 = std::fs::read_to_string(dir.join("pid"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    alive(p).then_some(p)
}

/// Reads the folder: stopped unless the daemon is alive.
pub fn read(dir: &Path) -> Dictation {
    if pid(dir).is_none() {
        return Dictation::Stopped;
    }
    std::fs::read_to_string(dir.join("state"))
        .map(|s| parse(&s))
        .unwrap_or(Dictation::Stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states() {
        assert_eq!(parse("idle\n"), Dictation::Idle);
        assert_eq!(parse("recording"), Dictation::Recording);
        assert_eq!(parse("transcribing"), Dictation::Transcribing);
        assert_eq!(parse("stopped"), Dictation::Stopped);
        assert_eq!(parse(""), Dictation::Stopped);
    }

    #[test]
    fn a_dead_daemon_is_stopped() {
        let d = std::env::temp_dir().join(format!("logbook-vox-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("state"), "recording").unwrap();
        std::fs::write(d.join("pid"), std::process::id().to_string()).unwrap();
        assert_eq!(read(&d), Dictation::Recording);
        // pid_max is at most 2^22, so this pid is never alive.
        std::fs::write(d.join("pid"), "999999999").unwrap();
        assert_eq!(read(&d), Dictation::Stopped);
        let _ = std::fs::remove_dir_all(&d);
    }
}
