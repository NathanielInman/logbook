//! Switchyard as a sibling: its socket answers `run` for the transcribe
//! task and `snapshot` for where that task stands. Only the fields Logbook
//! reads are modeled, so a newer Switchyard still parses.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub label: String,
    pub phase: String,
    pub message: Option<String>,
    pub queue_pos: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Mainline {
    pub current: Option<String>,
    pub queue: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Snapshot {
    pub tasks: Vec<Task>,
    pub mainline: Mainline,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
struct Reply {
    ok: bool,
    error: Option<String>,
    message: Option<String>,
    snapshot: Option<Snapshot>,
}

impl Snapshot {
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn label(&self, id: &str) -> String {
        self.task(id)
            .map(|t| {
                if t.label.is_empty() {
                    t.id.clone()
                } else {
                    t.label.clone()
                }
            })
            .unwrap_or_else(|| id.to_string())
    }

    /// What `id` waits for: the build on the main line and anything queued
    /// ahead, or the reason Switchyard holds it.
    pub fn behind(&self, id: &str) -> Option<String> {
        let t = self.task(id)?;
        match t.phase.as_str() {
            "queued" => {
                let mut ahead: Vec<String> = vec![];
                if let Some(c) = self.mainline.current.as_deref().filter(|c| *c != id) {
                    ahead.push(self.label(c));
                }
                for q in &self.mainline.queue {
                    if q == id {
                        break;
                    }
                    ahead.push(self.label(q));
                }
                (!ahead.is_empty()).then(|| ahead.join(", "))
            }
            "waiting" => Some(t.message.clone().unwrap_or_else(|| "free memory".into())),
            _ => None,
        }
    }
}

fn call(line: &str) -> anyhow::Result<Reply> {
    let path = crate::paths::switchyard_socket();
    let mut s = crate::transport::connect_to(&path, Some(Duration::from_secs(3)))?;
    s.write_all(line.as_bytes())?;
    s.write_all(b"\n")?;
    let mut out = String::new();
    BufReader::new(s).read_line(&mut out)?;
    Ok(serde_json::from_str(&out)?)
}

/// Switchyard's snapshot, when it runs.
pub fn snapshot() -> Option<Snapshot> {
    call(r#"{"cmd":"snapshot"}"#).ok()?.snapshot
}

/// Asks Switchyard to run `id` with `params`. Its message ("already
/// queued") comes back as an error, since nothing new was started.
pub fn run(id: &str, params: &BTreeMap<String, String>) -> anyhow::Result<()> {
    let line = serde_json::json!({"cmd": "run", "id": id, "params": params}).to_string();
    let r = call(&line)?;
    if !r.ok {
        anyhow::bail!(r.error.unwrap_or_else(|| "Switchyard said no".into()));
    }
    match r.message {
        Some(m) if m.contains("already") => anyhow::bail!(m),
        _ => Ok(()),
    }
}

/// Asks Switchyard to read its manifests again.
pub fn reload() -> bool {
    call(r#"{"cmd":"reload"}"#).is_ok_and(|r| r.ok)
}

/// The task `logbook setup` writes to Switchyard's `tasks.d`.
pub fn manifest(task: &str, logbook: &str) -> String {
    format!(
        "# Written by `logbook setup`. Logbook queues each finished meeting here,\n\
         # so a transcript waits for the main line like any heavy build.\n\
         \n\
         [task.{task}]\n\
         label = \"logbook transcribe\"\n\
         description = \"a meeting into transcript.md\"\n\
         dir = \"~\"\n\
         tags = [\"logbook\"]\n\
         heavy = true\n\
         shelf = true\n\
         notify = \"failure\"\n\
         steps = [\"{logbook} transcribe '{{dir}}'\"]\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAP: &str = r#"{"tasks":[
        {"id":"det33","label":"det33 build","phase":"building","extra":1},
        {"id":"forge","label":"","phase":"queued"},
        {"id":"logbook-transcribe","label":"logbook transcribe","phase":"queued"},
        {"id":"held","phase":"waiting","message":"held: Roundhouse says /data is 96% full"}
    ],"mainline":{"current":"det33","queue":["forge","logbook-transcribe"]},"now":1.0}"#;

    #[test]
    fn what_a_transcript_waits_for() {
        let s: Snapshot = serde_json::from_str(SNAP).unwrap();
        assert_eq!(
            s.behind("logbook-transcribe").as_deref(),
            Some("det33 build, forge")
        );
        assert_eq!(
            s.behind("held").as_deref(),
            Some("held: Roundhouse says /data is 96% full")
        );
        assert_eq!(s.behind("det33"), None);
        assert_eq!(s.behind("missing"), None);
    }

    #[test]
    fn manifest_is_valid_toml() {
        let m = manifest("logbook-transcribe", "/home/n/.local/bin/logbook");
        let v: toml::Value = toml::from_str(&m).unwrap();
        let t = &v["task"]["logbook-transcribe"];
        assert_eq!(t["heavy"].as_bool(), Some(true));
        assert_eq!(
            t["steps"][0].as_str(),
            Some("/home/n/.local/bin/logbook transcribe '{dir}'")
        );
    }
}
