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

/// The events in one inotify read: the watch, the mask and the name.
fn events(buf: &[u8]) -> Vec<(i32, u32, String)> {
    let head = std::mem::size_of::<libc::inotify_event>();
    let mut out = vec![];
    let mut i = 0;
    while i + head <= buf.len() {
        // SAFETY: the kernel wrote a whole inotify_event here; it may be
        // unaligned within the byte buffer.
        let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf[i..].as_ptr().cast()) };
        let start = i + head;
        let end = (start + ev.len as usize).min(buf.len());
        let raw = &buf[start..end];
        let raw = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        out.push((ev.wd, ev.mask, String::from_utf8_lossy(raw).into_owned()));
        i = end;
    }
    out
}

/// What a batch of events means for the history folder.
#[derive(Default)]
struct Seen {
    changed: bool,
    /// The folder, or one on the way to it, appeared or went away.
    rewatch: bool,
}

fn classify(fd: i32, dir_wd: i32, up_wd: i32, next: &str, seen: &mut Seen) -> bool {
    let mut buf = [0u8; 8192];
    let mut any = false;
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return any;
        }
        any = true;
        for (wd, mask, name) in events(&buf[..n as usize]) {
            if wd == dir_wd && dir_wd >= 0 {
                if mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED) != 0 {
                    seen.rewatch = true;
                    seen.changed = true;
                } else if logbook_core::voxtype::is_day_file(&name) {
                    seen.changed = true;
                }
            } else if wd == up_wd && up_wd >= 0 && name == next {
                seen.rewatch = true;
                seen.changed = true;
            }
        }
    }
}

/// Calls `send` whenever voxtype's history changes: a day file written,
/// replaced (a correction or a delete rewrites the whole day) or removed,
/// or the folder appearing or going. The `.lock` that every `voxtype
/// history` command takes, Logbook's own reads included, is not a change.
/// A burst of events (a rewrite is several) sends once, after it settles.
pub fn watch_history(send: impl Fn()) {
    let dir = logbook_core::paths::voxtype_history_dir();
    let mut first = true;
    loop {
        let ino = Fd(unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) });
        if ino.0 < 0 {
            eprintln!("logbook: inotify: {}", std::io::Error::last_os_error());
            std::thread::sleep(Duration::from_secs(30));
            continue;
        }
        // The nearest folder above the history that exists, watched for the
        // next one down to appear (voxtype makes them on its first entry).
        let mut up = dir.parent();
        let mut next = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        while let Some(p) = up {
            if p.is_dir() {
                break;
            }
            next = p.file_name().map(|n| n.to_string_lossy().into_owned());
            up = p.parent();
        }
        let next = next.unwrap_or_default();
        let up_wd = up.map_or(-1, |p| {
            add_watch(
                ino.0,
                p,
                libc::IN_CREATE
                    | libc::IN_DELETE
                    | libc::IN_MOVED_TO
                    | libc::IN_MOVED_FROM
                    | libc::IN_ONLYDIR,
            )
        });
        let dir_wd = if dir.is_dir() {
            add_watch(
                ino.0,
                &dir,
                libc::IN_CLOSE_WRITE
                    | libc::IN_MOVED_TO
                    | libc::IN_MOVED_FROM
                    | libc::IN_DELETE
                    | libc::IN_DELETE_SELF
                    | libc::IN_MOVE_SELF,
            )
        } else {
            -1
        };
        if up_wd < 0 && dir_wd < 0 {
            // Nothing on the way exists, not even the data folder; the
            // drawer opening still asks voxtype.
            return;
        }
        // Whatever happened while the watches were down.
        if !first {
            send();
        }
        first = false;
        loop {
            let mut pfd = libc::pollfd {
                fd: ino.0,
                events: libc::POLLIN,
                revents: 0,
            };
            let n = unsafe { libc::poll(&mut pfd, 1, -1) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            let mut seen = Seen::default();
            classify(ino.0, dir_wd, up_wd, &next, &mut seen);
            if !seen.changed {
                continue;
            }
            // Let the burst finish: quiet for 200 ms.
            loop {
                pfd.revents = 0;
                if unsafe { libc::poll(&mut pfd, 1, 200) } <= 0 {
                    break;
                }
                classify(ino.0, dir_wd, up_wd, &next, &mut seen);
            }
            if seen.rewatch {
                break;
            }
            send();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_event_names() {
        let head = std::mem::size_of::<libc::inotify_event>();
        let mut buf = vec![];
        for (wd, name) in [(3, "2026-11-23.jsonl"), (4, "")] {
            let padded = if name.is_empty() {
                0
            } else {
                (name.len() + 1).div_ceil(16) * 16
            };
            let ev = libc::inotify_event {
                wd,
                mask: libc::IN_CLOSE_WRITE,
                cookie: 0,
                len: padded as u32,
            };
            let bytes: [u8; 16] = unsafe { std::mem::transmute(ev) };
            assert_eq!(head, bytes.len());
            buf.extend_from_slice(&bytes);
            let mut n = name.as_bytes().to_vec();
            n.resize(padded, 0);
            buf.extend_from_slice(&n);
        }
        let got = events(&buf);
        assert_eq!(
            got,
            vec![
                (3, libc::IN_CLOSE_WRITE, "2026-11-23.jsonl".into()),
                (4, libc::IN_CLOSE_WRITE, String::new())
            ]
        );
    }

    #[test]
    fn a_history_change_wakes_the_watch_and_the_lock_does_not() {
        let base = std::env::temp_dir().join(format!("logbook-hist-{}", std::process::id()));
        let dir = base.join("voxtype").join("history");
        std::fs::create_dir_all(&dir).unwrap();
        let ino = Fd(unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) });
        let wd = add_watch(
            ino.0,
            &dir,
            libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_DELETE,
        );
        assert!(wd >= 0);

        std::fs::write(dir.join(".lock"), "").unwrap();
        let mut seen = Seen::default();
        classify(ino.0, wd, -1, "", &mut seen);
        assert!(!seen.changed);

        std::fs::write(dir.join(".tmp-2026-11-23.jsonl-1"), "{}\n").unwrap();
        std::fs::rename(
            dir.join(".tmp-2026-11-23.jsonl-1"),
            dir.join("2026-11-23.jsonl"),
        )
        .unwrap();
        let mut seen = Seen::default();
        classify(ino.0, wd, -1, "", &mut seen);
        assert!(seen.changed && !seen.rewatch);
        let _ = std::fs::remove_dir_all(&base);
    }
}
