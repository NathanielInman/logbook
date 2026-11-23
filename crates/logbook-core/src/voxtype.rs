//! voxtype's daemon state, from the files it keeps in
//! `$XDG_RUNTIME_DIR/voxtype`: `state` holds one word (`idle`, `recording`,
//! `transcribing`) and `pid` the daemon's process. The daemon watches the
//! folder with inotify, so dictation needs no `voxtype status` child.
//!
//! voxtype 1.3 also keeps a history of every dictation. Logbook treats
//! voxtype as its backend there: it reads `voxtype history list --json`
//! rather than the day files, which voxtype gzips after a week and guards
//! with its own privacy controls, and it changes history only through
//! voxtype's `history` commands.

use crate::model::{Dictated, Dictation, History};
use serde::Deserialize;
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

/// `voxtype history list --json`. Only the fields Logbook shows; voxtype
/// adds more, which are ignored.
#[derive(Deserialize)]
struct ListJson {
    /// Always there in an answer; an `{"error": ...}` object lacks it.
    enabled: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    retention_days: u32,
    #[serde(default)]
    count: u32,
    #[serde(default)]
    entries: Vec<EntryJson>,
}

#[derive(Deserialize)]
struct EntryJson {
    id: String,
    ts_start: String,
    #[serde(default)]
    audio_ms: u64,
    #[serde(default)]
    raw_text: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    corrected_text: Option<String>,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    delivered: bool,
    #[serde(default)]
    error: Option<String>,
}

/// Reads what `voxtype history list --json` printed. `None` when it is not
/// that (an error object, or an older voxtype's usage text).
pub fn history(json: &str) -> Option<History> {
    let list: ListJson = serde_json::from_str(json.trim()).ok()?;
    let entries = list
        .entries
        .into_iter()
        .filter_map(|e| {
            let started = crate::time::rfc3339(&e.ts_start)?;
            // What `voxtype history copy` sends: the correction, else the
            // output, else what the engine heard.
            let (text, corrected) = match e.corrected_text {
                Some(c) => (c, true),
                None if !e.text.trim().is_empty() => (e.text, false),
                None => (e.raw_text, false),
            };
            Some(Dictated {
                id: e.id,
                started,
                length: e.audio_ms as f64 / 1000.0,
                text: text.trim().to_string(),
                corrected,
                app: e.app.filter(|a| !a.trim().is_empty()),
                outcome: e
                    .outcome
                    .unwrap_or_else(|| if e.delivered { "delivered" } else { "failed" }.into()),
                error: e.error,
            })
        })
        .collect();
    Some(History {
        known: true,
        enabled: list.enabled,
        paused: list.paused,
        retention_days: list.retention_days,
        count: list.count,
        entries,
    })
}

/// A file in voxtype's history folder that holds dictations:
/// `2026-11-23.jsonl`, or `2026-11-01.jsonl.gz` once it is a week old. Its
/// `.lock` and the temporary files of a rewrite are not.
pub fn is_day_file(name: &str) -> bool {
    let stem = name
        .strip_suffix(".jsonl.gz")
        .or_else(|| name.strip_suffix(".jsonl"));
    stem.is_some_and(|d| {
        let b = d.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    })
}

/// How a dictation that did not reach its target ended, for a list.
pub fn outcome_words(outcome: &str) -> &str {
    match outcome {
        "delivered" => "",
        "cancelled" => "cancelled",
        "no_speech" => "no speech heard",
        "too_short" => "too short",
        "empty" => "nothing heard",
        "output_failed" => "could not type it",
        "transcription_failed" => "transcription failed",
        _ => "failed",
    }
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

    /// What voxtype 1.3.1 printed for `history list --json -n 5` over three
    /// dictations: one typed, one that heard nothing, one corrected.
    const LIST: &str = r#"{"count":3,"enabled":true,"entries":[{"app":"code","audio_ms":10750,"auto_submit":false,"corrected_text":"Hyprland workspaces","delivered":true,"ended_by":"released","engine":"whisper","id":"20261123-112030-0f9e8d","model":"large-v3-turbo","outcome":"delivered","raw_text":"hyper land workspaces","schema":1,"streaming":false,"text":"hyper land workspaces","trigger":"record","ts_end":"2026-11-23T11:20:41.250-06:00","ts_start":"2026-11-23T11:20:30.500-06:00","voxtype_version":"1.3.1"},{"app":"google-chrome","audio_ms":1119,"auto_submit":false,"delivered":false,"ended_by":"released","engine":"whisper","id":"20261123-101744-d4e5f6","model":"large-v3-turbo","outcome":"no_speech","raw_text":"","schema":1,"streaming":false,"text":"","trigger":"hotkey","ts_end":"2026-11-23T10:17:45.120-06:00","ts_start":"2026-11-23T10:17:44.001-06:00","voxtype_version":"1.3.1"},{"app":"kitty","audio_ms":7484,"auto_submit":false,"backend":"vulkan","delivered":true,"edits":[{"from":"send","to":"Send"}],"ended_by":"released","engine":"whisper","id":"20261123-091502-a1b2c3","model":"large-v3-turbo","outcome":"delivered","output":{"driver":"wtype","fell_back":false,"mode":"type"},"raw_text":"send the invoice to marta by friday","schema":1,"streaming":false,"text":"Send the invoice to Marta by Friday.","transcribe_ms":612,"trigger":"hotkey","ts_end":"2026-11-23T09:15:09.902-06:00","ts_start":"2026-11-23T09:15:02.418-06:00","voxtype_version":"1.3.1"}],"paused":false,"recording":true,"retention_days":30}"#;

    #[test]
    fn history_list() {
        let h = history(LIST).unwrap();
        assert!(h.known && h.enabled && !h.paused && h.keeping());
        assert_eq!((h.count, h.retention_days, h.entries.len()), (3, 30, 3));

        let fixed = &h.entries[0];
        assert_eq!(fixed.id, "20261123-112030-0f9e8d");
        assert_eq!(fixed.text, "Hyprland workspaces");
        assert!(fixed.corrected && fixed.delivered());
        assert_eq!(fixed.app.as_deref(), Some("code"));
        assert!((fixed.length - 10.75).abs() < 1e-9);
        assert!((fixed.started - 1_795_454_430.5).abs() < 1e-6);

        let silent = &h.entries[1];
        assert_eq!(silent.text, "");
        assert!(!silent.delivered());
        assert_eq!(outcome_words(&silent.outcome), "no speech heard");

        let typed = &h.entries[2];
        assert_eq!(typed.text, "Send the invoice to Marta by Friday.");
        assert!(!typed.corrected);
    }

    #[test]
    fn history_off_empty_or_missing() {
        let h = history(
            r#"{"count":0,"enabled":false,"entries":[],"paused":false,"recording":false,"retention_days":30}"#,
        )
        .unwrap();
        assert!(h.known && !h.enabled && !h.keeping());
        let h = history(
            r#"{"count":0,"enabled":true,"entries":[],"paused":true,"recording":false,"retention_days":0}"#,
        )
        .unwrap();
        assert!(h.paused && !h.keeping());
        assert_eq!(history(r#"{"error":"history is broken"}"#), None);
        assert_eq!(history("error: unrecognized subcommand 'history'"), None);
        assert_eq!(history(""), None);
    }

    #[test]
    fn a_failed_dictation_keeps_what_the_engine_heard() {
        let h = history(
            r#"{"enabled":true,"entries":[{"id":"20261123-120000-aaaaaa","ts_start":"2026-11-23T12:00:00Z","raw_text":"lost words","text":"","delivered":false,"outcome":"output_failed","error":"wtype exited 1"}]}"#,
        )
        .unwrap();
        let e = &h.entries[0];
        assert_eq!(e.text, "lost words");
        assert_eq!(e.error.as_deref(), Some("wtype exited 1"));
        assert_eq!(outcome_words(&e.outcome), "could not type it");
    }

    #[test]
    fn day_files() {
        assert!(is_day_file("2026-11-23.jsonl"));
        assert!(is_day_file("2026-11-01.jsonl.gz"));
        assert!(!is_day_file(".lock"));
        assert!(!is_day_file(".tmp-2026-11-14.jsonl-999"));
        assert!(!is_day_file("2026-11-23.json"));
        assert!(!is_day_file("notes.jsonl"));
    }
}
