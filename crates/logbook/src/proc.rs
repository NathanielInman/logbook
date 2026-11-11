//! Child processes the daemon owns. Each one gets its own process group,
//! so a stop takes down everything it started (the old meeting script
//! signalled a group that did not exist and left its loop recording), and a
//! parent-death signal, so a daemon that dies however it dies takes its
//! recorders with it.
//!
//! The kernel sends the parent-death signal when the *thread* that forked
//! the child exits, not the process. Every child is therefore forked on one
//! thread that lives as long as the daemon.

use anyhow::{anyhow, Result};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

type Job = (Command, i32, Sender<std::io::Result<Child>>);

static OWNER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

fn owner() -> Sender<Job> {
    OWNER
        .get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            std::thread::Builder::new()
                .name("logbook-owner".into())
                .spawn(move || {
                    for (mut cmd, sig, back) in rx {
                        unsafe {
                            cmd.pre_exec(move || {
                                reset_signals();
                                if libc::setpgid(0, 0) != 0 {
                                    return Err(std::io::Error::last_os_error());
                                }
                                libc::prctl(libc::PR_SET_PDEATHSIG, sig as libc::c_ulong);
                                // The daemon may have died between fork and
                                // prctl; then nobody will send the signal.
                                if libc::getppid() == 1 {
                                    libc::_exit(1);
                                }
                                Ok(())
                            });
                        }
                        let _ = back.send(cmd.spawn());
                    }
                })
                .expect("owner thread");
            Mutex::new(tx)
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Gives a child a clean signal state. The daemon blocks SIGTERM, SIGINT
/// and SIGHUP in every thread so one thread can `sigwait` for them, and a
/// blocked mask survives exec: a recorder would then ignore the SIGINT that
/// asks it to close its file. A daemon started in the background by a
/// shell also inherits SIGINT ignored.
///
/// Only async-signal-safe calls: this runs between fork and exec.
fn reset_signals() {
    unsafe {
        let mut empty: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        libc::pthread_sigmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
        for s in [
            libc::SIGINT,
            libc::SIGTERM,
            libc::SIGHUP,
            libc::SIGQUIT,
            libc::SIGPIPE,
        ] {
            libc::signal(s, libc::SIG_DFL);
        }
    }
}

/// A command whose child starts with a clean signal state.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    unsafe {
        c.pre_exec(|| {
            reset_signals();
            Ok(())
        });
    }
    c
}

/// A child in its own process group.
pub struct Owned {
    pub child: Child,
}

impl Owned {
    pub fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// Signals the whole group.
    pub fn signal(&self, sig: i32) {
        unsafe {
            libc::kill(-self.pid(), sig);
        }
    }

    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Asks the group to end with `sig`, waits up to `grace`, then kills
    /// it. Returns the leader's exit status when it had one.
    pub fn stop(&mut self, sig: i32, grace: Duration) -> Option<ExitStatus> {
        self.signal(sig);
        let until = Instant::now() + grace;
        let status = loop {
            if let Some(s) = self.try_wait() {
                break Some(s);
            }
            if Instant::now() >= until {
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        // Anything the leader started that is still in the group.
        self.signal(libc::SIGKILL);
        match status {
            Some(s) => Some(s),
            None => self.child.wait().ok(),
        }
    }
}

impl Owned {
    /// Waits up to `grace` for a child that is about to exit on its own,
    /// then ends the group.
    pub fn reap(mut self, grace: Duration) {
        let until = Instant::now() + grace;
        while self.try_wait().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if self.try_wait().is_none() {
            self.signal(libc::SIGKILL);
            let _ = self.child.wait();
        }
    }
}

/// Starts `cmd` in its own group. `death` is the signal it gets if the
/// daemon goes away: SIGINT for recorders, so they close their files.
pub fn spawn(cmd: Command, death: i32) -> Result<Owned> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let (tx, rx) = channel();
    owner()
        .send((cmd, death, tx))
        .map_err(|_| anyhow!("the owner thread is gone"))?;
    let child = rx
        .recv()
        .map_err(|_| anyhow!("the owner thread is gone"))?
        .map_err(|e| anyhow!("cannot start {name}: {e}"))?;
    Ok(Owned { child })
}

/// Whether any process is left in group `pgid`.
pub fn group_alive(pgid: i32) -> bool {
    pgid > 1 && unsafe { libc::kill(-pgid, 0) } == 0
}

/// Ends a group left behind by a daemon that crashed, if it is still
/// there: SIGINT first so a recorder can close its file.
pub fn end_group(pgid: i32) {
    if !group_alive(pgid) {
        return;
    }
    unsafe {
        libc::kill(-pgid, libc::SIGINT);
    }
    for _ in 0..40 {
        if !group_alive(pgid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

/// Runs a short command to completion and returns its stdout, or `None` if
/// it failed or took longer than `timeout`.
pub fn output(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = command(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let until = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                use std::io::Read;
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return s.success().then_some(out);
            }
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Starts a fire-and-forget helper (notify-send, xdg-open) and reaps it.
pub fn detach(program: &str, args: &[&str]) {
    let child = command(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut c) = child {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_takes_the_whole_group() {
        // A shell that starts a grandchild, like the old chunk loop did.
        let out = std::env::temp_dir().join(format!("logbook-proc-{}", std::process::id()));
        let mut c = Command::new("sh");
        c.args([
            "-c",
            &format!("sleep 30 & echo $! > {}; wait", out.display()),
        ]);
        let mut o = spawn(c, libc::SIGTERM).unwrap();
        let pgid = o.pid();
        let mut grandchild = 0;
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(&out) {
                if let Ok(p) = s.trim().parse::<i32>() {
                    grandchild = p;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(grandchild > 0);
        assert!(group_alive(pgid));
        o.stop(libc::SIGTERM, Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!group_alive(pgid), "the group outlived stop");
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn children_start_with_signals_unblocked() {
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        }
        let out = output(
            "grep",
            &["SigBlk", "/proc/self/status"],
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(out.trim().ends_with("0000000000000000"), "{out}");
        let mut c = Command::new("sh");
        c.args(["-c", "grep SigBlk /proc/self/status"]);
        c.stdout(std::process::Stdio::piped());
        let mut o = spawn(c, libc::SIGTERM).unwrap();
        let mut text = String::new();
        use std::io::Read;
        o.child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.trim().ends_with("0000000000000000"), "{text}");
    }

    #[test]
    fn short_commands() {
        assert_eq!(
            output("echo", &["hi"], Duration::from_secs(2)).as_deref(),
            Some("hi\n")
        );
        assert_eq!(output("false", &[], Duration::from_secs(2)), None);
        assert_eq!(output("sleep", &["5"], Duration::from_millis(100)), None);
    }
}
