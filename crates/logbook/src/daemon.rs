//! The daemon: owns the recorders, follows PipeWire and voxtype, keeps the
//! transcript queue, serves the socket, feeds the bar module, and starts the
//! drawer the first time it is asked for.
//!
//! Recorders are children in their own process groups (see `proc`), so a
//! stop ends everything they started, and a daemon that dies takes them
//! with it. What is recording lives in `$XDG_RUNTIME_DIR/logbook/live.json`,
//! which a reboot clears; a daemon that finds one from a crashed daemon ends
//! its groups and finishes its meeting.
//!
//! Nothing here polls for state. The clock ticks once a second only while
//! something records, and Switchyard is asked how a transcript is doing
//! only while one is out with it.

use crate::proc::{self, Owned};
use anyhow::{anyhow, Result};
use logbook_core::config::Config;
use logbook_core::model::{
    self, Dictation, Job, JobState, Kind, Meeting, MicUser, Recording, Screen, ScreenPhase,
    Snapshot, Transcript,
};
use logbook_core::protocol::{Event, Reply, Request, SubKind, What};
use logbook_core::transport::{Listener, Stream};
use logbook_core::{library, now, paths, time, yard};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// How often the config file is checked for changes.
const CONFIG_EVERY: Duration = Duration::from_secs(2);
/// How often Switchyard is asked about a transcript it holds.
const YARD_EVERY: Duration = Duration::from_secs(3);
/// A recorder that dies sooner than this after starting has failed.
const QUICK_DEATH: f64 = 3.0;

struct MeetingRun {
    info: Meeting,
    rec: Owned,
    chunk_started: f64,
    quick_deaths: u32,
}

struct ScreenRun {
    info: Screen,
    rec: Option<Owned>,
}

/// What `live.json` holds: enough for a new daemon to clean up after a
/// crashed one.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Live {
    #[serde(default)]
    meeting: Option<String>,
    #[serde(default)]
    meeting_pgid: Option<i32>,
    #[serde(default)]
    screen: Option<String>,
    #[serde(default)]
    screen_pgid: Option<i32>,
    #[serde(default)]
    transcribe_pgid: Option<i32>,
    #[serde(default)]
    dnd_held: bool,
}

struct State {
    cfg: Config,
    cfg_error: Option<String>,
    cfg_mtime: Option<SystemTime>,
    meeting: Option<MeetingRun>,
    screen: Option<ScreenRun>,
    dictation: Dictation,
    mic: Vec<MicUser>,
    mic_known: bool,
    jobs: Vec<Job>,
    /// A transcript run without Switchyard.
    direct: Option<Owned>,
    /// When Switchyard last showed the job's task busy, so a task that
    /// ends without the worker reporting is noticed.
    yard_seen_busy: bool,
    recordings: Vec<Recording>,
    dnd_held: bool,
    problem: Option<String>,
    snap: Snapshot,
    boards: Vec<Stream>,
    bars: Vec<Stream>,
    bar_line: String,
    pending: Option<Event>,
    board_started: Option<Instant>,
    visible: bool,
    /// Our recorders' pids, for telling our streams from others'.
    our_pids: Vec<u32>,
    graph: Option<crate::pw::Graph>,
}

pub struct Daemon {
    state: Mutex<State>,
    cv: Condvar,
    /// Serializes do not disturb changes, which run outside the state lock.
    dnd: Mutex<()>,
}

type Shared = Arc<Daemon>;

impl Daemon {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn send_line(s: &mut Stream, line: &str) -> bool {
    s.write_all(line.as_bytes())
        .and_then(|_| s.write_all(b"\n"))
        .and_then(|_| s.flush())
        .is_ok()
}

fn broadcast(subs: &mut Vec<Stream>, line: &str) -> usize {
    subs.retain_mut(|s| send_line(s, line));
    subs.len()
}

fn event_line(e: &Event) -> String {
    serde_json::to_string(e).unwrap_or_default()
}

fn config_mtime() -> Option<SystemTime> {
    std::fs::metadata(paths::config_file())
        .and_then(|m| m.modified())
        .ok()
}

fn notify(title: &str, body: &str, icon: &str) {
    proc::detach("notify-send", &["-a", "Logbook", "-i", icon, title, body]);
}

// ---------------------------------------------------------------- snapshot

fn transcript_state(jobs: &[Job], r: &Recording) -> Transcript {
    if r.kind != Kind::Meeting {
        return Transcript::None;
    }
    match jobs.iter().find(|j| j.dir == r.path) {
        Some(j) => match j.state {
            JobState::Queued => Transcript::Queued,
            JobState::Running => Transcript::Running,
            JobState::Failed => Transcript::Failed,
        },
        None => r.transcript,
    }
}

/// Recomputes the snapshot and pushes what changed.
fn refresh(st: &mut State) {
    let t = now();
    let mut recordings = st.recordings.clone();
    for r in &mut recordings {
        r.transcript = transcript_state(&st.jobs, r);
    }
    let mut snap = Snapshot {
        version: env!("CARGO_PKG_VERSION").into(),
        now: t,
        lamp: Default::default(),
        meeting: st.meeting.as_ref().map(|m| m.info.clone()),
        screen: st.screen.as_ref().map(|s| s.info.clone()),
        dictation: st.dictation,
        mic: st.mic.clone(),
        mic_known: st.mic_known,
        jobs: st.jobs.clone(),
        recordings,
        dnd_held: st.dnd_held,
        config_error: st.cfg_error.clone(),
        problem: st.problem.clone(),
    };
    snap.lamp = model::lamp(&snap);
    let bar = serde_json::to_string(&crate::bar::line(&snap, &st.cfg)).unwrap_or_default();
    if bar != st.bar_line {
        st.bar_line = bar.clone();
        broadcast(&mut st.bars, &bar);
    }
    let ev = event_line(&Event::Snapshot {
        snapshot: Box::new(snap.clone()),
    });
    broadcast(&mut st.boards, &ev);
    st.snap = snap;
}

fn write_live(st: &State) {
    let live = Live {
        meeting: st.meeting.as_ref().map(|m| m.info.dir.clone()),
        meeting_pgid: st.meeting.as_ref().map(|m| m.rec.pid()),
        screen: st.screen.as_ref().map(|s| s.info.path.clone()),
        screen_pgid: st
            .screen
            .as_ref()
            .and_then(|s| s.rec.as_ref())
            .map(|r| r.pid()),
        transcribe_pgid: st.direct.as_ref().map(|r| r.pid()),
        dnd_held: st.dnd_held,
    };
    let path = paths::live_file();
    if live.meeting.is_none()
        && live.screen.is_none()
        && live.transcribe_pgid.is_none()
        && !live.dnd_held
    {
        let _ = std::fs::remove_file(path);
        return;
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_string(&live).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

fn update_pids(st: &mut State) {
    let mut v = vec![];
    if let Some(m) = &st.meeting {
        v.push(m.rec.pid() as u32);
    }
    if let Some(r) = st.screen.as_ref().and_then(|s| s.rec.as_ref()) {
        v.push(r.pid() as u32);
    }
    st.our_pids = v;
    recompute_mic(st);
}

fn recompute_mic(st: &mut State) {
    let Some(g) = &st.graph else {
        st.mic.clear();
        st.mic_known = false;
        return;
    };
    let cfg = &st.cfg;
    st.mic = if cfg.mic.watch {
        logbook_core::mic::users(&g.nodes, &g.links, &g.client_pids, &st.our_pids, |b, a| {
            cfg.ignored(b, a)
        })
    } else {
        vec![]
    };
    st.mic_known = true;
}

/// Lists the recordings on disk again, off the lock.
fn rescan(d: &Shared) {
    let d = d.clone();
    std::thread::spawn(move || {
        let (meetings, screens, limit, skip) = {
            let st = d.lock();
            let mut skip = vec![];
            if let Some(m) = &st.meeting {
                skip.push(m.info.dir.clone());
            }
            if let Some(s) = &st.screen {
                skip.push(s.info.path.clone());
            }
            (
                st.cfg.meeting_dir(),
                st.cfg.screen_dir(),
                st.cfg.drawer.recent.max(1),
                skip,
            )
        };
        let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
        let list = library::scan(&meetings, &screens, limit, &skip);
        let mut st = d.lock();
        st.recordings = list;
        refresh(&mut st);
    });
}

// ---------------------------------------------------------- do not disturb

fn swaync(arg: &str) -> Option<bool> {
    let out = proc::output("swaync-client", &[arg, "-sw"], Duration::from_secs(3))?;
    match out.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Holds do not disturb on while a recording that wants it runs, and gives
/// it back after. Do not disturb that was already on is left alone.
fn sync_dnd(d: &Shared) {
    let _g = d.dnd.lock().unwrap_or_else(|e| e.into_inner());
    let (want, held) = {
        let st = d.lock();
        let want = (st.meeting.is_some() && st.cfg.meeting.dnd)
            || (st.screen.is_some() && st.cfg.screen.dnd);
        (want, st.dnd_held)
    };
    if want && !held {
        if swaync("-D") == Some(false) && swaync("-dn") == Some(true) {
            let mut st = d.lock();
            st.dnd_held = true;
            write_live(&st);
            refresh(&mut st);
        }
    } else if !want && held {
        swaync("-df");
        let mut st = d.lock();
        st.dnd_held = false;
        write_live(&st);
        refresh(&mut st);
    }
}

// ------------------------------------------------------------------ meeting

fn pw_record(cfg: &Config, path: &Path) -> Result<Owned> {
    let mut cmd = Command::new("pw-record");
    cmd.args(["--rate", "16000", "--channels", "1", "--format", "s16"]);
    if !cfg.meeting.source.trim().is_empty() {
        cmd.args(["--target", cfg.meeting.source.trim()]);
    }
    cmd.arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    proc::spawn(cmd, libc::SIGINT)
}

fn start_meeting(d: &Shared) -> Result<String> {
    {
        let mut st = d.lock();
        if st.meeting.is_some() {
            return Ok("already recording a meeting".into());
        }
        let t = now();
        let dir = st.cfg.meeting_dir().join(time::meeting_name(t));
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow!("cannot create {}: {e}", dir.display()))?;
        let chunk = dir.join(time::chunk_name(1, t));
        let rec = pw_record(&st.cfg, &chunk)?;
        st.meeting = Some(MeetingRun {
            info: Meeting {
                dir: dir.display().to_string(),
                started: t,
                chunk: 1,
                chunk_path: chunk.display().to_string(),
                marks: vec![],
                source: if st.cfg.meeting.source.is_empty() {
                    "default input".into()
                } else {
                    st.cfg.meeting.source.clone()
                },
            },
            rec,
            chunk_started: t,
            quick_deaths: 0,
        });
        st.problem = None;
        update_pids(&mut st);
        write_live(&st);
        refresh(&mut st);
        d.cv.notify_all();
    }
    sync_dnd(d);
    Ok("recording a meeting".into())
}

/// Starts the next chunk, then stops the one before, so no audio falls in
/// between.
fn rotate(st: &mut State) {
    let Some(m) = st.meeting.as_mut() else {
        return;
    };
    let t = now();
    let n = m.info.chunk + 1;
    let path = PathBuf::from(&m.info.dir).join(time::chunk_name(n, t));
    match pw_record(&st.cfg, &path) {
        Ok(rec) => {
            let mut old = std::mem::replace(&mut m.rec, rec);
            m.info.chunk = n;
            m.info.chunk_path = path.display().to_string();
            m.chunk_started = t;
            std::thread::spawn(move || {
                old.stop(libc::SIGINT, Duration::from_secs(3));
            });
        }
        Err(e) => st.problem = Some(format!("{e:#}")),
    }
    update_pids(st);
    write_live(st);
}

fn stop_meeting(d: &Shared) -> Option<String> {
    let run = {
        let mut st = d.lock();
        let run = st.meeting.take()?;
        update_pids(&mut st);
        write_live(&st);
        refresh(&mut st);
        run
    };
    let MeetingRun { info, mut rec, .. } = run;
    rec.stop(libc::SIGINT, Duration::from_secs(3));
    drop(rec);
    sync_dnd(d);
    let dir = PathBuf::from(&info.dir);
    let r = library::meeting(&dir);
    let (chunks, len) = r.map(|r| (r.chunks, r.length)).unwrap_or((0, 0.0));
    let transcribe = d.lock().cfg.meeting.transcribe;
    let body = format!(
        "{} \u{b7} {chunks} chunk{} \u{b7} {}",
        time::length(len),
        if chunks == 1 { "" } else { "s" },
        paths::tilde(&dir)
    );
    notify("Meeting saved", &body, "audio-input-microphone");
    if transcribe && chunks > 0 {
        enqueue(d, &info.dir);
    }
    rescan(d);
    Some("meeting stopped".into())
}

fn mark(d: &Shared) -> Result<String> {
    let mut st = d.lock();
    let Some(m) = st.meeting.as_mut() else {
        return Err(anyhow!("no meeting is recording"));
    };
    let at = now() - m.info.started;
    m.info.marks.push(at);
    let n = m.info.marks.len();
    library::write_marks(Path::new(&m.info.dir), &m.info.marks)?;
    refresh(&mut st);
    Ok(format!("mark {n} at {}", time::clock(at)))
}

// ------------------------------------------------------------------- screen

fn audio_source(cfg: &Config) -> Option<String> {
    match cfg.screen.audio.trim() {
        "none" | "" => None,
        "monitor" => proc::output("pactl", &["get-default-sink"], Duration::from_secs(3))
            .map(|s| format!("{}.monitor", s.trim()))
            .filter(|s| s != ".monitor"),
        other => Some(other.to_string()),
    }
}

fn start_screen(d: &Shared, geometry: Option<String>) -> Result<String> {
    let geometry = geometry.filter(|g| !g.trim().is_empty()).ok_or_else(|| {
        anyhow!("a screen recording needs a region (logbook start screen asks slurp)")
    })?;
    let cfg = d.lock().cfg.clone();
    let audio = audio_source(&cfg);
    {
        let mut st = d.lock();
        if st.screen.is_some() {
            return Ok("already recording the screen".into());
        }
        let dir = st.cfg.screen_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow!("cannot create {}: {e}", dir.display()))?;
        let t = now();
        let path = dir.join(time::screen_name(t));
        let mut cmd = Command::new("wf-recorder");
        cmd.args(["-y", "-g", geometry.trim()]);
        if let Some(a) = &audio {
            cmd.arg(format!("--audio={a}"));
        }
        cmd.arg("-f")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let rec = proc::spawn(cmd, libc::SIGINT)?;
        st.screen = Some(ScreenRun {
            info: Screen {
                path: path.display().to_string(),
                started: t,
                phase: ScreenPhase::Recording,
                geometry: geometry.trim().to_string(),
                processing_since: None,
            },
            rec: Some(rec),
        });
        st.problem = None;
        update_pids(&mut st);
        write_live(&st);
        refresh(&mut st);
        d.cv.notify_all();
    }
    sync_dnd(d);
    Ok("recording the screen".into())
}

/// Stops wf-recorder, then runs the loudness pass with the lamp amber.
fn stop_screen(d: &Shared) -> Option<String> {
    let (mut rec, path, loudnorm) = {
        let mut st = d.lock();
        let s = st.screen.as_mut()?;
        if s.info.phase == ScreenPhase::Processing {
            return Some("the screen capture is already being saved".into());
        }
        let rec = s.rec.take()?;
        s.info.phase = ScreenPhase::Processing;
        s.info.processing_since = Some(now());
        let path = s.info.path.clone();
        let loudnorm = st.cfg.screen.loudnorm;
        update_pids(&mut st);
        refresh(&mut st);
        (rec, path, loudnorm)
    };
    // wf-recorder writes the MP4 index on SIGINT; give it time.
    rec.stop(libc::SIGINT, Duration::from_secs(10));
    drop(rec);
    let d2 = d.clone();
    std::thread::spawn(move || {
        if loudnorm && Path::new(&path).exists() {
            if let Err(e) = normalize(Path::new(&path)) {
                d2.lock().problem = Some(format!("loudness pass: {e:#}"));
            }
        }
        {
            let mut st = d2.lock();
            st.screen = None;
            update_pids(&mut st);
            write_live(&st);
            refresh(&mut st);
        }
        sync_dnd(&d2);
        let len = logbook_core::media::read_mp4(Path::new(&path))
            .map(time::length)
            .unwrap_or_default();
        notify(
            "Screen recording saved",
            &format!("{len} \u{b7} {}", paths::tilde(Path::new(&path))),
            "video-x-generic",
        );
        rescan(&d2);
    });
    Some("screen recording stopped".into())
}

/// `ffmpeg -af loudnorm -c:v copy`, written beside the capture and moved
/// over it when it worked.
fn normalize(path: &Path) -> Result<()> {
    let tmp = path.with_extension("normalized.mp4");
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-y", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-af", "loudnorm", "-c:v", "copy"])
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut job = proc::spawn(cmd, libc::SIGTERM)?;
    let status = job.child.wait()?;
    if status.success() && tmp.exists() {
        std::fs::rename(&tmp, path)?;
        Ok(())
    } else {
        let _ = std::fs::remove_file(&tmp);
        Err(anyhow!("ffmpeg exited with {status}"))
    }
}

// -------------------------------------------------------------- transcripts

fn save_queue(jobs: &[Job]) {
    let _ = std::fs::create_dir_all(paths::state_dir());
    let v: Vec<Job> = jobs
        .iter()
        .map(|j| Job {
            state: if j.state == JobState::Running {
                JobState::Queued
            } else {
                j.state
            },
            runner: None,
            done: 0,
            done_secs: 0.0,
            started: None,
            behind: None,
            ..j.clone()
        })
        .collect();
    let path = paths::queue_file();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

fn load_queue() -> Vec<Job> {
    std::fs::read_to_string(paths::queue_file())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Job>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|j| Path::new(&j.dir).is_dir())
        .collect()
}

fn enqueue(d: &Shared, dir: &str) {
    {
        let mut st = d.lock();
        match st.jobs.iter_mut().find(|j| j.dir == dir) {
            Some(j) if j.state == JobState::Failed => {
                *j = Job {
                    dir: dir.to_string(),
                    ..Default::default()
                };
            }
            Some(_) => return,
            None => st.jobs.push(Job {
                dir: dir.to_string(),
                ..Default::default()
            }),
        }
        save_queue(&st.jobs);
        refresh(&mut st);
    }
    pump(d);
}

/// Starts the next queued transcript when none is running.
fn pump(d: &Shared) {
    let (dir, cfg) = {
        let mut st = d.lock();
        if st.jobs.iter().any(|j| j.state == JobState::Running) {
            return;
        }
        let Some(j) = st.jobs.iter_mut().find(|j| j.state == JobState::Queued) else {
            return;
        };
        j.state = JobState::Running;
        j.error = None;
        let dir = j.dir.clone();
        st.yard_seen_busy = false;
        refresh(&mut st);
        (dir, st.cfg.clone())
    };
    let d = d.clone();
    std::thread::spawn(move || {
        let t = &cfg.transcribe;
        let use_yard = match t.runner.as_str() {
            "direct" => false,
            "switchyard" => true,
            _ => yard::snapshot().is_some_and(|s| s.task(&t.task).is_some()),
        };
        let started = if use_yard {
            let params = BTreeMap::from([("dir".to_string(), dir.clone())]);
            yard::run(&t.task, &params).map(|_| "switchyard")
        } else {
            start_direct(&d, &dir, &cfg).map(|_| "direct")
        };
        let mut st = d.lock();
        let Some(j) = st.jobs.iter_mut().find(|j| j.dir == dir) else {
            return;
        };
        let failed = started.is_err();
        match started {
            Ok(runner) => {
                j.runner = Some(runner.into());
                if runner == "switchyard" {
                    j.behind = Some("Switchyard".into());
                }
            }
            Err(e) => {
                j.state = JobState::Failed;
                j.error = Some(format!("{e:#}"));
            }
        }
        refresh(&mut st);
        drop(st);
        d.cv.notify_all();
        if failed {
            pump(&d);
        }
    });
}

/// Runs `logbook transcribe <dir>` under `transcribe.wrap` (lowprio), as a
/// child in its own group.
fn start_direct(d: &Shared, dir: &str, cfg: &Config) -> Result<()> {
    let exe = std::env::current_exe()?;
    let wrap: Vec<&String> = cfg
        .transcribe
        .wrap
        .iter()
        .filter(|w| !w.trim().is_empty())
        .collect();
    let usable = wrap.first().is_some_and(|w| paths::which(w).is_some());
    let mut cmd = if usable {
        let mut c = Command::new(wrap[0]);
        c.args(&wrap[1..]).arg(&exe);
        c
    } else {
        // No lowprio: nice keeps it behind the desktop at least.
        let mut c = Command::new("nice");
        c.args(["-n", "19"]).arg(&exe);
        c
    };
    cmd.arg("transcribe").arg(dir).stdin(Stdio::null());
    let _ = std::fs::create_dir_all(paths::state_dir());
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::state_dir().join("transcribe.log"))
    {
        if let Ok(f2) = f.try_clone() {
            cmd.stdout(f).stderr(f2);
        }
    }
    let owned = proc::spawn(cmd, libc::SIGTERM)?;
    let mut st = d.lock();
    st.direct = Some(owned);
    write_live(&st);
    Ok(())
}

/// A transcript ended, by the worker's report or because its process or
/// Switchyard task did.
fn finish(d: &Shared, dir: &str, ok: bool, error: Option<String>) {
    {
        let mut st = d.lock();
        let Some(i) = st.jobs.iter().position(|j| j.dir == dir) else {
            return;
        };
        if st.jobs[i].state != JobState::Running {
            return;
        }
        // The worker reports just before it exits.
        if let Some(o) = st.direct.take() {
            std::thread::spawn(move || o.reap(Duration::from_secs(5)));
        }
        write_live(&st);
        if ok {
            st.jobs.remove(i);
        } else {
            st.jobs[i].state = JobState::Failed;
            st.jobs[i].error = Some(error.unwrap_or_else(|| "the transcriber failed".into()));
            st.jobs[i].behind = None;
        }
        save_queue(&st.jobs);
        refresh(&mut st);
    }
    let name = Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if ok {
        notify("Transcript ready", &name, "text-x-generic");
    } else {
        notify("Transcript failed", &name, "dialog-error");
    }
    rescan(d);
    pump(d);
}

/// Checks on a transcript held by Switchyard or run directly.
fn watch_job(d: &Shared) {
    let (job, cfg) = {
        let mut st = d.lock();
        let Some(job) = st
            .jobs
            .iter()
            .find(|j| j.state == JobState::Running)
            .cloned()
        else {
            return;
        };
        // A direct run that ended without reporting.
        if job.runner.as_deref() == Some("direct") {
            let status = st.direct.as_mut().and_then(|o| o.try_wait());
            if let Some(s) = status {
                st.direct = None;
                write_live(&st);
                drop(st);
                let ok = s.success() && library::transcript(Path::new(&job.dir)).is_some();
                finish(
                    d,
                    &job.dir,
                    ok,
                    (!ok).then(|| format!("logbook transcribe exited with {s}; see ~/.local/state/logbook/transcribe.log")),
                );
            }
            return;
        }
        (job, st.cfg.clone())
    };
    if job.runner.as_deref() != Some("switchyard") {
        return;
    }
    let Some(snap) = yard::snapshot() else {
        finish(
            d,
            &job.dir,
            false,
            Some("Switchyard stopped before the transcript was done".into()),
        );
        return;
    };
    let Some(task) = snap.task(&cfg.transcribe.task) else {
        finish(
            d,
            &job.dir,
            false,
            Some(format!("Switchyard has no task {}", cfg.transcribe.task)),
        );
        return;
    };
    let busy = matches!(task.phase.as_str(), "queued" | "waiting" | "building");
    let behind = snap.behind(&cfg.transcribe.task);
    let mut st = d.lock();
    if busy {
        st.yard_seen_busy = true;
        if let Some(j) = st.jobs.iter_mut().find(|j| j.dir == job.dir) {
            if j.behind != behind {
                j.behind = behind;
                refresh(&mut st);
            }
        }
        return;
    }
    if !st.yard_seen_busy {
        return;
    }
    // The task ended and the worker never said so (killed, out of memory).
    drop(st);
    let ok = task.phase == "ok" && library::transcript(Path::new(&job.dir)).is_some();
    let msg = task
        .message
        .clone()
        .unwrap_or_else(|| "the Switchyard task failed; its log has the details".into());
    finish(d, &job.dir, ok, (!ok).then_some(msg));
}

// ------------------------------------------------------------------- ticker

/// Ticks the clock and rotates chunks while recording, watches the
/// recorders, checks on transcripts and picks up config changes.
fn ticker(d: Shared) {
    let mut config_at = Instant::now();
    let mut yard_at = Instant::now();
    loop {
        let mut st = d.lock();
        let recording = st.meeting.is_some() || st.screen.is_some();
        let job = st.jobs.iter().any(|j| j.state == JobState::Running);
        let wait = if recording {
            Duration::from_secs(1)
        } else if job {
            YARD_EVERY
        } else {
            CONFIG_EVERY
        };
        st =
            d.cv.wait_timeout(st, wait)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        let t = now();

        // The meeting: next chunk, or a recorder that died.
        let mut fail_meeting = None;
        let mut next_chunk = false;
        let chunk = st.cfg.meeting.chunk.max(10) as f64;
        if let Some(m) = st.meeting.as_mut() {
            if let Some(status) = m.rec.try_wait() {
                let quick = t - m.chunk_started < QUICK_DEATH;
                m.quick_deaths = if quick { m.quick_deaths + 1 } else { 0 };
                if m.quick_deaths >= 2 {
                    fail_meeting = Some(format!("pw-record keeps exiting ({status})"));
                } else {
                    next_chunk = true;
                }
            } else if t - m.chunk_started >= chunk {
                next_chunk = true;
            }
        }
        if next_chunk {
            rotate(&mut st);
        }
        // wf-recorder that ended on its own (the output went away).
        let screen_died = st
            .screen
            .as_mut()
            .and_then(|s| s.rec.as_mut())
            .and_then(|r| r.try_wait())
            .is_some();
        if config_at.elapsed() >= CONFIG_EVERY {
            config_at = Instant::now();
            let m = config_mtime();
            if m != st.cfg_mtime {
                st.cfg_mtime = m;
                let (cfg, err) = Config::load();
                st.cfg = cfg;
                st.cfg_error = err;
                recompute_mic(&mut st);
            }
        }
        refresh(&mut st);
        let check_job = job && yard_at.elapsed() >= YARD_EVERY;
        drop(st);

        if let Some(e) = fail_meeting {
            d.lock().problem = Some(e);
            stop_meeting(&d);
        }
        if screen_died {
            stop_screen(&d);
        }
        if check_job {
            yard_at = Instant::now();
            let d = d.clone();
            std::thread::spawn(move || watch_job(&d));
        }
    }
}

// -------------------------------------------------------------------- board

fn spawn_board() {
    let exe = std::env::current_exe().ok();
    let local = exe
        .as_ref()
        .and_then(|e| e.parent())
        .map(|dir| dir.join("logbook-board"))
        .filter(|p| p.exists());
    let bin = local
        .map(|p| p.into_os_string())
        .unwrap_or_else(|| "logbook-board".into());
    let log = paths::board_log();
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut cmd = proc::command(&bin);
    cmd.stdin(Stdio::null());
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
    {
        if let Ok(f2) = f.try_clone() {
            cmd.stdout(f).stderr(f2);
        }
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("logbook: cannot start {}: {e}", bin.to_string_lossy()),
    }
}

fn to_board(st: &mut State, ev: Event) {
    if broadcast(&mut st.boards, &event_line(&ev)) > 0 {
        return;
    }
    let show = match ev {
        Event::Toggle { anchor } | Event::Show { anchor } => Event::Show { anchor },
        other => other,
    };
    if matches!(show, Event::Hide) {
        return;
    }
    st.pending = Some(show);
    if st
        .board_started
        .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
    {
        st.board_started = Some(Instant::now());
        spawn_board();
    }
}

// ------------------------------------------------------------------- socket

fn reply_of(r: Result<String>) -> Reply {
    match r {
        Ok(m) => Reply::msg(m),
        Err(e) => Reply::err(format!("{e:#}")),
    }
}

fn stop(d: &Shared, what: Option<What>) -> Reply {
    let mut said = vec![];
    if matches!(what, None | Some(What::Meeting)) {
        if let Some(m) = stop_meeting(d) {
            said.push(m);
        }
    }
    if matches!(what, None | Some(What::Screen)) {
        if let Some(m) = stop_screen(d) {
            said.push(m);
        }
    }
    if said.is_empty() {
        Reply::msg(match what {
            Some(What::Meeting) => "no meeting is recording",
            Some(What::Screen) => "the screen is not recording",
            None => "nothing is recording",
        })
    } else {
        Reply::msg(said.join(", "))
    }
}

fn handle(d: Shared, stream: Stream) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    let reader = BufReader::new(read_half);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = send_line(
                    &mut writer,
                    &serde_json::to_string(&Reply::err(format!("bad request: {e}")))
                        .unwrap_or_default(),
                );
                continue;
            }
        };
        let reply = match req {
            Request::Ping => Reply {
                version: Some(env!("CARGO_PKG_VERSION").into()),
                ..Reply::ok()
            },
            Request::Snapshot => {
                let mut st = d.lock();
                refresh(&mut st);
                Reply {
                    snapshot: Some(Box::new(st.snap.clone())),
                    ..Reply::ok()
                }
            }
            Request::Subscribe { kind } => {
                let _ = writer.set_write_timeout(Some(Duration::from_secs(2)));
                let mut st = d.lock();
                match kind {
                    SubKind::Board => {
                        let mut ok = send_line(
                            &mut writer,
                            &event_line(&Event::Snapshot {
                                snapshot: Box::new(st.snap.clone()),
                            }),
                        );
                        if let Some(p) = st.pending.take() {
                            ok &= send_line(&mut writer, &event_line(&p));
                        }
                        if ok {
                            st.boards.push(writer);
                        }
                    }
                    SubKind::Bar => {
                        let line = st.bar_line.clone();
                        if line.is_empty() || send_line(&mut writer, &line) {
                            st.bars.push(writer);
                        }
                    }
                }
                return;
            }
            Request::Toggle { anchor } => {
                to_board(&mut d.lock(), Event::Toggle { anchor });
                Reply::ok()
            }
            Request::Show { anchor } => {
                to_board(&mut d.lock(), Event::Show { anchor });
                Reply::ok()
            }
            Request::Hide => {
                let mut st = d.lock();
                st.pending = None;
                broadcast(&mut st.boards, &event_line(&Event::Hide));
                Reply::ok()
            }
            Request::BoardVisible { visible } => {
                let mut st = d.lock();
                st.visible = visible;
                drop(st);
                if visible {
                    rescan(&d);
                }
                Reply::ok()
            }
            Request::Start { what, geometry } => reply_of(match what {
                What::Meeting => start_meeting(&d),
                What::Screen => start_screen(&d, geometry),
            }),
            Request::Stop { what } => stop(&d, what),
            Request::Mark => reply_of(mark(&d)),
            Request::Queue { dir } => {
                let p = paths::expand(&dir);
                if library::chunks(&p).is_empty() {
                    Reply::err(format!("{} has no chunk-*.wav files", paths::tilde(&p)))
                } else {
                    enqueue(&d, &p.display().to_string());
                    Reply::msg("queued for a transcript")
                }
            }
            Request::Forget { dir } => {
                let mut st = d.lock();
                let before = st.jobs.len();
                st.jobs
                    .retain(|j| j.dir != dir || j.state == JobState::Running);
                save_queue(&st.jobs);
                refresh(&mut st);
                if st.jobs.len() < before {
                    Reply::ok()
                } else {
                    Reply::err("nothing to forget (a running transcript cannot be dropped)")
                }
            }
            Request::Rescan => {
                rescan(&d);
                Reply::ok()
            }
            Request::Progress {
                dir,
                done,
                total,
                done_secs,
                total_secs,
            } => {
                let mut st = d.lock();
                if let Some(j) = st
                    .jobs
                    .iter_mut()
                    .find(|j| j.dir == dir && j.state == JobState::Running)
                {
                    j.done = done;
                    j.total = total;
                    j.done_secs = done_secs;
                    j.total_secs = total_secs;
                    j.behind = None;
                    if j.started.is_none() {
                        j.started = Some(now());
                    }
                    refresh(&mut st);
                }
                Reply::ok()
            }
            Request::Finished { dir, ok, error } => {
                let d2 = d.clone();
                std::thread::spawn(move || finish(&d2, &dir, ok, error));
                Reply::ok()
            }
        };
        if !send_line(
            &mut writer,
            &serde_json::to_string(&reply).unwrap_or_default(),
        ) {
            return;
        }
    }
}

// --------------------------------------------------------------- lifecycle

/// Cleans up after a daemon that crashed: ends its recorders' groups,
/// gives back do not disturb, and queues the meeting it was recording.
fn recover(d: &Shared) {
    let Some(live) = std::fs::read_to_string(paths::live_file())
        .ok()
        .and_then(|t| serde_json::from_str::<Live>(&t).ok())
    else {
        return;
    };
    eprintln!("logbook: cleaning up after a daemon that did not stop cleanly");
    for pgid in [live.meeting_pgid, live.screen_pgid, live.transcribe_pgid]
        .into_iter()
        .flatten()
    {
        proc::end_group(pgid);
    }
    if live.dnd_held {
        swaync("-df");
    }
    let _ = std::fs::remove_file(paths::live_file());
    if let Some(dir) = live.meeting {
        let transcribe = d.lock().cfg.meeting.transcribe;
        if transcribe && !library::chunks(Path::new(&dir)).is_empty() {
            enqueue(d, &dir);
        }
    }
}

/// Stops everything on SIGTERM, SIGINT or SIGHUP and exits, so a stopped
/// service or a logout never leaves a recorder running or do not disturb
/// on.
fn shutdown(d: &Shared, sig: i32) -> ! {
    eprintln!("logbook: stopping on signal {sig}");
    let meeting = d.lock().meeting.take();
    if let Some(mut m) = meeting {
        m.rec.stop(libc::SIGINT, Duration::from_secs(3));
    }
    let screen = d.lock().screen.take();
    if let Some(mut s) = screen {
        if let Some(mut r) = s.rec.take() {
            r.stop(libc::SIGINT, Duration::from_secs(5));
        }
    }
    let direct = d.lock().direct.take();
    if let Some(mut t) = direct {
        t.stop(libc::SIGTERM, Duration::from_secs(2));
    }
    if d.lock().dnd_held {
        swaync("-df");
    }
    {
        let st = d.lock();
        save_queue(&st.jobs);
    }
    let _ = std::fs::remove_file(paths::live_file());
    let _ = std::fs::remove_file(paths::socket_path());
    std::process::exit(0);
}

fn block_signals() -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

pub fn run() -> Result<i32> {
    // Blocked before any thread starts, so only the signal thread takes
    // them. std's process spawn empties the mask in each child.
    let set = block_signals();
    let listener = Listener::bind()?;
    let (cfg, cfg_error) = Config::load();
    if let Some(e) = &cfg_error {
        eprintln!("logbook: {e}");
    }
    let jobs = load_queue();
    let d: Shared = Arc::new(Daemon {
        state: Mutex::new(State {
            cfg,
            cfg_error,
            cfg_mtime: config_mtime(),
            meeting: None,
            screen: None,
            dictation: Dictation::Stopped,
            mic: vec![],
            mic_known: false,
            jobs,
            direct: None,
            yard_seen_busy: false,
            recordings: vec![],
            dnd_held: false,
            problem: None,
            snap: Snapshot::default(),
            boards: vec![],
            bars: vec![],
            bar_line: String::new(),
            pending: None,
            board_started: None,
            visible: false,
            our_pids: vec![],
            graph: None,
        }),
        cv: Condvar::new(),
        dnd: Mutex::new(()),
    });
    {
        let d = d.clone();
        std::thread::spawn(move || loop {
            let mut sig: libc::c_int = 0;
            if unsafe { libc::sigwait(&set, &mut sig) } == 0 {
                shutdown(&d, sig);
            }
        });
    }
    recover(&d);
    {
        let mut st = d.lock();
        refresh(&mut st);
    }
    rescan(&d);
    {
        let d = d.clone();
        std::thread::Builder::new()
            .name("logbook-pipewire".into())
            .spawn(move || {
                crate::pw::watch(move |g| {
                    let mut st = d.lock();
                    if st.graph == g {
                        return;
                    }
                    st.graph = g;
                    let before = st.mic.clone();
                    let known = st.mic_known;
                    recompute_mic(&mut st);
                    if st.mic != before || st.mic_known != known {
                        refresh(&mut st);
                    }
                })
            })?;
    }
    {
        let d = d.clone();
        std::thread::spawn(move || {
            crate::voxwatch::watch(|state| {
                let mut st = d.lock();
                if st.dictation != state {
                    st.dictation = state;
                    refresh(&mut st);
                }
            })
        });
    }
    {
        let d = d.clone();
        std::thread::spawn(move || ticker(d));
    }
    pump(&d);
    eprintln!("logbook: listening on {}", paths::socket_path().display());
    loop {
        match listener.accept() {
            Ok(stream) => {
                let d = d.clone();
                std::thread::spawn(move || handle(d, stream));
            }
            Err(e) => {
                eprintln!("logbook: accept failed: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}
