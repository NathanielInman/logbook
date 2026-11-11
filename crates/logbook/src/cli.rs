//! Command-line entry points.

use anyhow::{anyhow, Result};
use logbook_core::config::Config;
use logbook_core::model::{Dictation, JobState, Kind, Snapshot, Transcript};
use logbook_core::protocol::{Request, SubKind, What};
use logbook_core::{library, now, paths, time, yard};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const HELP: &str = "\
logbook: an On Air lamp for waybar, with meetings, screen captures,
dictation and transcripts behind it

Usage:
  logbook toggle [--center]           open or close the drawer from the clicked icon
  logbook show [--center] | hide      open or close the drawer
  logbook toggle meeting|screen       start or stop a recording (screen asks slurp
                                      for a region)
  logbook start meeting|screen        start a recording
  logbook stop [meeting|screen]       stop one recording, or everything
  logbook mark                        mark this moment in the meeting; the
                                      transcript gets a heading here
  logbook dictate                     start or stop voxtype dictation
  logbook queue <dir>                 queue a meeting folder for a transcript
  logbook transcribe <dir>            transcribe a meeting folder now (what the
                                      queue runs)
  logbook list [--json]               recent recordings
  logbook copy [dir]                  copy a transcript (default: the newest)
  logbook bar                         the waybar module (prints JSON lines)
  logbook status [--json]             what is recording and who has the mic
  logbook doctor                      check the setup
  logbook setup [--systemd]           add the transcribe task to Switchyard, and
                                      with --systemd a user service
  logbook daemon                      run the daemon in the foreground
  logbook --version

Config: ~/.config/logbook/config.toml
";

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

pub fn run(args: &[String]) -> Result<i32> {
    let rest = args.get(1..).unwrap_or(&[]);
    let what = || -> Result<Option<What>> {
        match rest.first() {
            None => Ok(None),
            Some(w) => What::parse(w)
                .map(Some)
                .ok_or_else(|| anyhow!("`{w}` is not meeting or screen")),
        }
    };
    match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            print!("{HELP}");
            Ok(0)
        }
        Some("-V" | "--version" | "version") => {
            println!("logbook {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some("daemon") => crate::daemon::run(),
        Some("bar") => bar(),
        Some("toggle") => match what()? {
            Some(w) => toggle_recording(w),
            None => open(true, rest),
        },
        Some("show") => open(false, rest),
        Some("hide") => {
            if logbook_core::transport::daemon_alive() {
                send(&Request::Hide)?;
            }
            Ok(0)
        }
        Some("start") => match what()? {
            Some(w) => start(w),
            None => Err(anyhow!("usage: logbook start meeting|screen")),
        },
        Some("stop") => {
            if !logbook_core::transport::daemon_alive() {
                println!("nothing is recording");
                return Ok(0);
            }
            say(Request::Stop { what: what()? })
        }
        Some("mark") => {
            ensure_daemon()?;
            say(Request::Mark)
        }
        Some("dictate") => {
            let s = std::process::Command::new("voxtype")
                .args(["record", "toggle"])
                .status()
                .map_err(|e| anyhow!("cannot run voxtype: {e}"))?;
            Ok(s.code().unwrap_or(1))
        }
        Some("queue") => {
            let dir = rest
                .first()
                .ok_or_else(|| anyhow!("usage: logbook queue <dir>"))?;
            let dir = paths::expand(dir);
            let dir = dir.canonicalize().unwrap_or(dir);
            ensure_daemon()?;
            say(Request::Queue {
                dir: dir.display().to_string(),
            })
        }
        Some("transcribe") => {
            let dir = rest
                .first()
                .ok_or_else(|| anyhow!("usage: logbook transcribe <dir>"))?;
            crate::transcribe::run(dir)
        }
        Some("list" | "ls") => list(flag(rest, "--json")),
        Some("copy") => copy(rest.first().map(String::as_str)),
        Some("status") => status(flag(rest, "--json")),
        Some("doctor") => doctor(),
        Some("setup") => setup(flag(rest, "--systemd")),
        Some(other) => Err(anyhow!("unknown command `{other}` (try `logbook --help`)")),
    }
}

fn send(req: &Request) -> Result<Option<String>> {
    let r = logbook_core::protocol::request(req)?;
    match r.error {
        Some(e) => Err(anyhow!(e)),
        None => Ok(r.message),
    }
}

/// Sends a request and prints what the daemon said.
fn say(req: Request) -> Result<i32> {
    if let Some(m) = send(&req)? {
        println!("{m}");
    }
    Ok(0)
}

fn unit_file() -> std::path::PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths::home().join(".config"))
        .join("systemd/user/logbook.service")
}

/// Starts the daemon when it is not running, through systemd when the user
/// service is installed, and waits for its socket.
fn ensure_daemon() -> Result<()> {
    if logbook_core::transport::daemon_alive() {
        return Ok(());
    }
    if unit_file().exists() {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "start", "--no-block", "logbook.service"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(50));
            if logbook_core::transport::daemon_alive() {
                return Ok(());
            }
        }
    }
    let exe = std::env::current_exe()?;
    let log = paths::daemon_log();
    std::fs::create_dir_all(paths::state_dir())?;
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out);
    {
        use std::os::unix::process::CommandExt;
        // Its own session, so restarting the bar does not take the daemon
        // (and a recording) with it.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn()?;
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(50));
        if logbook_core::transport::daemon_alive() {
            std::thread::spawn(move || child.wait());
            return Ok(());
        }
        if let Ok(Some(_)) = child.try_wait() {
            continue;
        }
    }
    Err(anyhow!("the daemon did not start; see {}", log.display()))
}

fn open(toggle: bool, args: &[String]) -> Result<i32> {
    let (cfg, _) = Config::load();
    // Ask the compositor first: the cursor is still on the icon.
    let anchor = bar_anchor::hypr::anchor(&cfg.bar.namespace, flag(args, "--center"));
    ensure_daemon()?;
    send(&if toggle {
        Request::Toggle { anchor }
    } else {
        Request::Show { anchor }
    })?;
    Ok(0)
}

/// Asks slurp for a region; `None` when the user pressed Escape.
fn region() -> Result<Option<String>> {
    let out = std::process::Command::new("slurp")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| anyhow!("cannot run slurp: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    let g = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!g.is_empty()).then_some(g))
}

fn start(w: What) -> Result<i32> {
    ensure_daemon()?;
    let geometry = match w {
        What::Meeting => None,
        What::Screen => {
            if snapshot().is_some_and(|s| s.screen.is_some()) {
                println!("already recording the screen");
                return Ok(0);
            }
            match region()? {
                Some(g) => Some(g),
                None => return Ok(0),
            }
        }
    };
    say(Request::Start { what: w, geometry })
}

fn toggle_recording(w: What) -> Result<i32> {
    ensure_daemon()?;
    let s = snapshot().ok_or_else(|| anyhow!("the daemon did not answer"))?;
    let on = match w {
        What::Meeting => s.meeting.is_some(),
        What::Screen => s.screen.is_some(),
    };
    if on {
        say(Request::Stop { what: Some(w) })
    } else {
        start(w)
    }
}

fn bar() -> Result<i32> {
    let offline = serde_json::to_string(&crate::bar::offline())?;
    let mut out = std::io::stdout().lock();
    let mut last = String::new();
    // Starts the daemon when it is missing (first run, a crash, an
    // upgrade), at most every 10 seconds.
    let mut started: Option<std::time::Instant> = None;
    loop {
        match logbook_core::protocol::subscribe_raw(SubKind::Bar) {
            Ok(lines) => {
                for line in lines {
                    let Ok(line) = line else { break };
                    if line != last {
                        writeln!(out, "{line}")?;
                        out.flush()?;
                        last = line;
                    }
                }
            }
            Err(_) if started.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) => {
                started = Some(std::time::Instant::now());
                let _ = ensure_daemon();
                continue;
            }
            Err(_) => {}
        }
        if last != offline {
            writeln!(out, "{offline}")?;
            out.flush()?;
            last = offline.clone();
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn snapshot() -> Option<Snapshot> {
    logbook_core::protocol::request(&Request::Snapshot)
        .ok()?
        .snapshot
        .map(|s| *s)
}

/// The recordings, from the daemon or, without one, from disk.
fn recordings() -> Vec<logbook_core::model::Recording> {
    if let Some(s) = snapshot() {
        return s.recordings;
    }
    let (cfg, _) = Config::load();
    library::scan(
        &cfg.meeting_dir(),
        &cfg.screen_dir(),
        cfg.drawer.recent.max(1),
        &[],
    )
}

fn transcript_word(t: Transcript) -> &'static str {
    match t {
        Transcript::None => "",
        Transcript::Missing => "no transcript",
        Transcript::Queued => "transcript queued",
        Transcript::Running => "transcribing",
        Transcript::Failed => "transcript failed",
        Transcript::Ready => "transcript ready",
    }
}

fn list(json: bool) -> Result<i32> {
    let list = recordings();
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(0);
    }
    if list.is_empty() {
        println!("No recordings yet.");
        return Ok(0);
    }
    let t = now();
    for r in &list {
        let kind = match r.kind {
            Kind::Meeting => "meeting",
            Kind::Screen => "screen ",
        };
        let mut extra = vec![time::length(r.length)];
        if r.marks > 0 {
            extra.push(format!(
                "{} mark{}",
                r.marks,
                if r.marks == 1 { "" } else { "s" }
            ));
        }
        let w = transcript_word(r.transcript);
        if !w.is_empty() {
            extra.push(w.into());
        }
        println!(
            "{kind}  {:<18} {:<36} {}",
            time::when(r.started, t),
            extra.join(" \u{b7} "),
            paths::tilde(Path::new(&r.path))
        );
    }
    Ok(0)
}

fn copy(dir: Option<&str>) -> Result<i32> {
    let path = match dir {
        Some(d) => library::transcript(&paths::expand(d))
            .ok_or_else(|| anyhow!("{d} has no transcript"))?,
        None => recordings()
            .into_iter()
            .find_map(|r| r.transcript_path.map(std::path::PathBuf::from))
            .ok_or_else(|| anyhow!("no meeting has a transcript yet"))?,
    };
    let text = std::fs::read(&path)?;
    let mut child = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("cannot run wl-copy: {e}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("wl-copy has no stdin"))?
        .write_all(&text)?;
    child.wait()?;
    println!("copied {}", paths::tilde(&path));
    Ok(0)
}

fn status(json: bool) -> Result<i32> {
    let Some(s) = snapshot() else {
        if json {
            println!("null");
        } else {
            println!(
                "The Logbook daemon is not running (`logbook bar` or `logbook daemon` starts it)."
            );
        }
        return Ok(1);
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(0);
    }
    let row = |k: &str, v: &str| println!("{k:<12}{v}");
    row("lamp", s.lamp.as_str());
    match &s.meeting {
        Some(m) => row(
            "meeting",
            &format!(
                "{} \u{b7} chunk {} \u{b7} {} mark(s) \u{b7} {}",
                time::clock(s.now - m.started),
                m.chunk,
                m.marks.len(),
                paths::tilde(Path::new(&m.dir))
            ),
        ),
        None => row("meeting", "not recording"),
    }
    match &s.screen {
        Some(x) => row(
            "screen",
            &format!(
                "{:?} \u{b7} {} \u{b7} {}",
                x.phase,
                time::clock(s.now - x.started),
                paths::tilde(Path::new(&x.path))
            )
            .to_lowercase(),
        ),
        None => row("screen", "not recording"),
    }
    row(
        "dictation",
        match s.dictation {
            Dictation::Stopped => "voxtype is not running",
            Dictation::Idle => "ready",
            Dictation::Recording => "listening",
            Dictation::Transcribing => "typing",
        },
    );
    if !s.mic_known {
        row("mic", "unknown (PipeWire is not answering)");
    } else if s.mic.is_empty() {
        row("mic", "closed");
    } else {
        for (i, u) in s.mic.iter().enumerate() {
            let who = if u.ours {
                " (Logbook)"
            } else if u.dictation {
                " (dictation)"
            } else {
                ""
            };
            row(
                if i == 0 { "mic" } else { "" },
                &format!("{}{who} on {}", u.name, u.source),
            );
        }
    }
    for j in &s.jobs {
        let name = Path::new(&j.dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let what = match j.state {
            JobState::Queued => "queued".to_string(),
            JobState::Running => {
                let mut w = format!("running via {}", j.runner.as_deref().unwrap_or("?"));
                if j.total > 0 {
                    w.push_str(&format!(" \u{b7} {}/{}", j.done, j.total));
                }
                if let Some(b) = &j.behind {
                    w.push_str(&format!(" \u{b7} waiting for {b}"));
                }
                w
            }
            JobState::Failed => format!("failed: {}", j.error.as_deref().unwrap_or("?")),
        };
        row("transcript", &format!("{name} {what}"));
    }
    if s.dnd_held {
        row("dnd", "on while recording (Logbook turns it off after)");
    }
    if let Some(p) = &s.problem {
        row("problem", p);
    }
    if let Some(e) = &s.config_error {
        println!("\nconfig: {e}");
    }
    Ok(0)
}

fn check(ok: Option<bool>, what: &str, detail: &str) {
    let tag = match ok {
        Some(true) => "ok  ",
        Some(false) => "fail",
        None => "info",
    };
    if detail.is_empty() {
        println!("{tag} {what}");
    } else {
        println!("{tag} {what}: {detail}");
    }
}

fn doctor() -> Result<i32> {
    let mut failed = false;
    let (cfg, err) = Config::load();
    match err {
        Some(e) => {
            failed = true;
            check(Some(false), "config", &e);
        }
        None if paths::config_file().exists() => check(
            Some(true),
            "config",
            &paths::config_file().display().to_string(),
        ),
        None => check(None, "config", "no config file, using defaults"),
    }
    match paths::xdg_runtime() {
        Some(d) => check(Some(true), "runtime dir", &d.display().to_string()),
        None => {
            failed = true;
            check(
                Some(false),
                "runtime dir",
                "XDG_RUNTIME_DIR is not set; state would outlive a reboot",
            )
        }
    }
    for (tool, why, needed) in [
        ("pw-record", "records meetings", true),
        ("wf-recorder", "records the screen", true),
        ("slurp", "picks the screen region", true),
        (
            "ffmpeg",
            "normalizes a capture's loudness",
            cfg.screen.loudnorm,
        ),
        ("swaync-client", "do not disturb while recording", false),
        (
            "notify-send",
            "says when a recording or transcript is saved",
            false,
        ),
        ("wl-copy", "copies transcripts", false),
        (
            "pactl",
            "finds the speakers' monitor for screen audio",
            cfg.screen.audio == "monitor",
        ),
    ] {
        match paths::which(tool) {
            Some(_) => check(Some(true), tool, why),
            None if needed => {
                failed = true;
                check(Some(false), tool, &format!("not found; it {why}"));
            }
            None => check(None, tool, &format!("not found; it {why}")),
        }
    }
    match cfg.transcribe.command.first() {
        Some(t) if paths::which(t).is_some() => {
            check(Some(true), "transcriber", &cfg.transcribe.command.join(" "))
        }
        Some(t) => {
            failed = true;
            check(Some(false), "transcriber", &format!("{t} not found"));
        }
        None => {
            failed = true;
            check(Some(false), "transcriber", "transcribe.command is empty");
        }
    }
    match logbook_core::paths::voxtype_dir() {
        Some(d) => {
            let s = logbook_core::voxtype::read(&d);
            check(
                (s != Dictation::Stopped).then_some(true),
                "voxtype",
                if s == Dictation::Stopped {
                    "the daemon is not running; dictation stays dark"
                } else {
                    "dictation lights the lamp"
                },
            );
        }
        None => check(None, "voxtype", "no runtime dir to read"),
    }
    match yard::snapshot() {
        Some(s) => {
            let has = s.task(&cfg.transcribe.task).is_some();
            check(
                Some(has || cfg.transcribe.runner == "direct"),
                "switchyard",
                &if has {
                    format!("transcripts queue as task {}", cfg.transcribe.task)
                } else {
                    format!(
                        "running, but has no task {} (run `logbook setup`)",
                        cfg.transcribe.task
                    )
                },
            );
        }
        None => check(
            None,
            "switchyard",
            "not running; transcripts run directly under lowprio",
        ),
    }
    let wrap = cfg
        .transcribe
        .wrap
        .first()
        .map(String::as_str)
        .unwrap_or("");
    if !wrap.is_empty() {
        check(
            paths::which(wrap).is_some().then_some(true),
            "wrap",
            &if paths::which(wrap).is_some() {
                format!("direct transcripts run under {wrap}")
            } else {
                format!("{wrap} not found; direct transcripts run under nice 19")
            },
        );
    }
    let alive = logbook_core::transport::daemon_alive();
    check(
        alive.then_some(true),
        "daemon",
        if alive {
            "running"
        } else {
            "not running (`logbook bar` or a command starts it)"
        },
    );
    if let Some(s) = snapshot() {
        check(
            Some(s.mic_known),
            "pipewire",
            if s.mic_known {
                "the lamp sees every app on the mic"
            } else {
                "not answering; only Logbook's own recordings light the lamp"
            },
        );
    }
    match bar_anchor::hypr::socket() {
        Some(_) => check(
            Some(true),
            "Hyprland IPC",
            "the drawer opens from the clicked icon",
        ),
        None => check(None, "Hyprland IPC", "not found; the drawer opens centered"),
    }
    match logbook_core::waybar::config_path() {
        Some(p) => {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let has = text.contains("logbook bar");
            check(
                Some(has),
                "waybar config",
                &format!(
                    "{}: {}",
                    paths::tilde(&p),
                    if has {
                        "has `logbook bar`"
                    } else {
                        "no module runs `logbook bar`"
                    }
                ),
            );
        }
        None => check(None, "waybar config", "not found"),
    }
    let board = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("logbook-board")))
        .filter(|p| p.exists());
    match board {
        Some(p) => check(Some(true), "drawer", &p.display().to_string()),
        None => check(
            None,
            "drawer",
            "logbook-board not next to logbook; it must be on PATH",
        ),
    }
    check(
        None,
        "recordings",
        &format!(
            "meetings in {}, captures in {}",
            paths::tilde(&cfg.meeting_dir()),
            paths::tilde(&cfg.screen_dir())
        ),
    );
    Ok(if failed { 1 } else { 0 })
}

const UNIT: &str = "\
[Unit]
Description=Logbook: the On Air lamp's daemon (recorders, mic watch, transcripts)
PartOf=graphical-session.target
After=graphical-session.target pipewire.service

[Service]
ExecStart={exe} daemon
# The daemon stops its recorders on SIGTERM and gives back do not disturb;
# only after that may systemd kill what is left.
KillMode=mixed
TimeoutStopSec=20
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
";

fn setup(systemd: bool) -> Result<i32> {
    let (cfg, _) = Config::load();
    let exe = std::env::current_exe()?;
    let exe_s = exe.display().to_string();
    let dir = paths::switchyard_tasks_dir();
    if dir.parent().is_some_and(|p| p.is_dir()) {
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("logbook.toml");
        std::fs::write(&file, yard::manifest(&cfg.transcribe.task, &exe_s))?;
        println!("wrote {}", paths::tilde(&file));
        if yard::reload() {
            println!("Switchyard reloaded its tasks");
        }
    } else {
        println!(
            "Switchyard is not set up ({} is missing); transcripts will run under lowprio",
            paths::tilde(dir.parent().unwrap_or(&dir))
        );
    }
    if systemd {
        let unit = unit_file();
        if let Some(d) = unit.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&unit, UNIT.replace("{exe}", &exe_s))?;
        println!("wrote {}", paths::tilde(&unit));
        let ok = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            // A daemon the bar started holds the lock; let systemd take over.
            if logbook_core::transport::daemon_alive()
                && snapshot().is_some_and(|s| s.meeting.is_none() && s.screen.is_none())
            {
                if let Some(pid) = daemon_pid() {
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "enable", "--now", "logbook.service"])
                .status();
        }
    }
    Ok(0)
}

/// The pid of the process holding the daemon socket.
fn daemon_pid() -> Option<i32> {
    use std::os::unix::io::AsRawFd;
    let s = logbook_core::transport::connect().ok()?;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (r == 0 && cred.pid > 0).then_some(cred.pid)
}
