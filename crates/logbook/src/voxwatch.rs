//! Follows voxtype's state file with inotify, and its daemon's life with a
//! pidfd, so dictation lights the lamp without a `voxtype status --follow`
//! child and without polling. When voxtype's folder does not exist yet the
//! watch is on `$XDG_RUNTIME_DIR` until it appears.

use logbook_core::model::Dictation;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

struct Fd(i32);

impl Drop for Fd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

fn add_watch(fd: i32, path: &Path, mask: u32) -> i32 {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return -1;
    };
    unsafe { libc::inotify_add_watch(fd, c.as_ptr(), mask) }
}

fn pidfd(pid: i32) -> Fd {
    Fd(unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 })
}

/// Drains whatever inotify has queued.
fn drain(fd: i32) {
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
    }
}

/// Calls `send` with voxtype's state whenever it changes, forever.
pub fn watch(send: impl Fn(Dictation)) {
    let Some(dir) = logbook_core::paths::voxtype_dir() else {
        send(Dictation::Stopped);
        return;
    };
    let parent = dir.parent().map(Path::to_path_buf);
    let mut last: Option<Dictation> = None;
    loop {
        let ino = Fd(unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) });
        if ino.0 < 0 {
            eprintln!("logbook: inotify: {}", std::io::Error::last_os_error());
            std::thread::sleep(Duration::from_secs(30));
            continue;
        }
        if let Some(p) = &parent {
            add_watch(
                ino.0,
                p,
                libc::IN_CREATE | libc::IN_DELETE | libc::IN_ONLYDIR,
            );
        }
        let watching_dir = add_watch(
            ino.0,
            &dir,
            libc::IN_CLOSE_WRITE
                | libc::IN_MOVED_TO
                | libc::IN_CREATE
                | libc::IN_DELETE
                | libc::IN_MODIFY
                | libc::IN_DELETE_SELF,
        ) >= 0;
        loop {
            let state = logbook_core::voxtype::read(&dir);
            if last != Some(state) {
                last = Some(state);
                send(state);
            }
            let pid = logbook_core::voxtype::pid(&dir);
            let pf = pid.map(pidfd).unwrap_or(Fd(-1));
            let mut fds = [
                libc::pollfd {
                    fd: ino.0,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: pf.0,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if fds[0].revents != 0 {
                drain(ino.0);
            }
            // The folder appeared or went away: watch it again.
            if !watching_dir || !dir.is_dir() {
                std::thread::sleep(Duration::from_millis(100));
                break;
            }
        }
    }
}
