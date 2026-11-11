//! The daemon socket: newline-delimited JSON, one request per line. The
//! shapes follow Roundhouse's, Switchyard's and Tannoy's (`{"cmd":"toggle",
//! "anchor":{...}}`, `{"event":"show",...}`), so a sibling can talk to any of
//! them without a shared crate.

use crate::model::Snapshot;
use crate::Anchor;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SubKind {
    /// Snapshots and show/hide events, for the drawer.
    Board,
    /// Waybar JSON lines, already formatted.
    Bar,
}

/// What a start or stop is about.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum What {
    Meeting,
    Screen,
}

impl What {
    pub fn parse(s: &str) -> Option<What> {
        match s {
            "meeting" | "meet" | "m" => Some(What::Meeting),
            "screen" | "s" => Some(What::Screen),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Snapshot,
    Subscribe {
        kind: SubKind,
    },
    Toggle {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Show {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Hide,
    /// The drawer reports whether it is on screen; the recordings on disk
    /// are listed again when it opens.
    BoardVisible {
        visible: bool,
    },
    /// Starts a recording. A screen capture needs the region, which the
    /// CLI asks slurp for before it sends this.
    Start {
        what: What,
        #[serde(default)]
        geometry: Option<String>,
    },
    /// Stops one recording, or everything Logbook records when `what` is
    /// missing.
    Stop {
        #[serde(default)]
        what: Option<What>,
    },
    /// Marks this moment in the meeting being recorded.
    Mark,
    /// Queues a meeting folder for a transcript.
    Queue {
        dir: String,
    },
    /// Drops a failed or queued transcript job.
    Forget {
        dir: String,
    },
    /// Lists the recordings on disk again.
    Rescan,
    /// Sent by `logbook transcribe` as it goes.
    Progress {
        dir: String,
        done: u32,
        total: u32,
        done_secs: f64,
        total_secs: f64,
    },
    /// Sent by `logbook transcribe` when it ends.
    Finished {
        dir: String,
        ok: bool,
        #[serde(default)]
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Box<Snapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Reply {
    pub fn ok() -> Reply {
        Reply {
            ok: true,
            ..Default::default()
        }
    }

    pub fn msg(m: impl ToString) -> Reply {
        Reply {
            ok: true,
            message: Some(m.to_string()),
            ..Default::default()
        }
    }

    pub fn err(e: impl ToString) -> Reply {
        Reply {
            ok: false,
            error: Some(e.to_string()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Snapshot {
        snapshot: Box<Snapshot>,
    },
    Show {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Toggle {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Hide,
}

/// The line a waybar `custom` module with `"return-type": "json"` reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BarLine {
    pub text: String,
    pub class: Vec<String>,
    pub alt: String,
    pub tooltip: String,
}

/// Sends one request and reads one reply, waiting at most `timeout`.
pub fn request_within(req: &Request, timeout: Duration) -> Result<Reply> {
    let path = crate::paths::socket_path();
    let mut s = crate::transport::connect_to(&path, Some(timeout))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(s);
    let mut out = String::new();
    reader.read_line(&mut out)?;
    serde_json::from_str(&out).map_err(|e| anyhow!("bad reply from daemon: {e}"))
}

/// Sends one request and reads one reply.
pub fn request(req: &Request) -> Result<Reply> {
    crate::transport::connect()?;
    request_within(req, Duration::from_secs(15))
}

/// Subscribes and yields raw lines until the daemon goes away.
pub fn subscribe_raw(kind: SubKind) -> Result<impl Iterator<Item = std::io::Result<String>>> {
    let mut s = crate::transport::connect()?;
    let mut line = serde_json::to_string(&Request::Subscribe { kind })?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    Ok(BufReader::new(s).lines())
}

/// Subscribes and yields parsed events.
pub fn subscribe(kind: SubKind) -> Result<impl Iterator<Item = Result<Event>>> {
    Ok(subscribe_raw(kind)?.map(|l| {
        let l = l?;
        serde_json::from_str::<Event>(&l).map_err(|e| anyhow!("bad event: {e}"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shapes() {
        let r: Request = serde_json::from_str(r#"{"cmd":"toggle"}"#).unwrap();
        assert_eq!(r, Request::Toggle { anchor: None });
        let s = serde_json::to_string(&Request::Subscribe { kind: SubKind::Bar }).unwrap();
        assert_eq!(s, r#"{"cmd":"subscribe","kind":"bar"}"#);
        let r: Request = serde_json::from_str(r#"{"cmd":"stop"}"#).unwrap();
        assert_eq!(r, Request::Stop { what: None });
        let r: Request =
            serde_json::from_str(r#"{"cmd":"start","what":"screen","geometry":"10,20 300x200"}"#)
                .unwrap();
        assert_eq!(
            r,
            Request::Start {
                what: What::Screen,
                geometry: Some("10,20 300x200".into())
            }
        );
        let s = serde_json::to_string(&Request::Mark).unwrap();
        assert_eq!(s, r#"{"cmd":"mark"}"#);
        let r: Request = serde_json::from_str(
            r#"{"cmd":"show","anchor":{"monitor":"DP-2","edge":"bottom","x":2310,"y":1428,"width":2560,"height":1440,"reserved":[0,0,0,24],"center":false}}"#,
        )
        .unwrap();
        assert!(matches!(r, Request::Show { anchor: Some(_) }));
    }

    #[test]
    fn event_shapes() {
        let e = serde_json::to_string(&Event::Hide).unwrap();
        assert_eq!(e, r#"{"event":"hide"}"#);
    }

    #[test]
    fn what_names() {
        assert_eq!(What::parse("meeting"), Some(What::Meeting));
        assert_eq!(What::parse("screen"), Some(What::Screen));
        assert_eq!(What::parse("dictation"), None);
    }
}
