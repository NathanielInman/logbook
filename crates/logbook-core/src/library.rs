//! The recordings on disk: a folder per meeting under `meeting.dir`, with
//! its chunks, marks and transcript, and a file per screen capture under
//! `screen.dir`. Folders the old `meeting-record.sh` made have the same
//! layout and list the same way.

use crate::model::{Kind, Recording, Transcript};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MARKS: &str = "marks.json";
pub const TRANSCRIPT: &str = "transcript.md";
/// What `meeting-record.sh transcribe` wrote.
pub const OLD_TRANSCRIPT: &str = "transcript.txt";

/// One mark in `marks.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mark {
    /// Seconds from the start of the meeting.
    pub at: f64,
}

/// The chunk files of a meeting, in order.
pub fn chunks(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<(u32, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let n = crate::time::chunk_number(name.to_str()?)?;
            Some((n, e.path()))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

pub fn read_marks(dir: &Path) -> Vec<f64> {
    let Ok(text) = std::fs::read_to_string(dir.join(MARKS)) else {
        return vec![];
    };
    let mut v: Vec<f64> = serde_json::from_str::<Vec<Mark>>(&text)
        .map(|m| m.into_iter().map(|m| m.at).collect())
        .unwrap_or_default();
    v.retain(|t| t.is_finite() && *t >= 0.0);
    v.sort_by(f64::total_cmp);
    v
}

pub fn write_marks(dir: &Path, marks: &[f64]) -> std::io::Result<()> {
    let v: Vec<Mark> = marks.iter().map(|&at| Mark { at }).collect();
    let body = serde_json::to_string_pretty(&v).unwrap_or_else(|_| "[]".into());
    let tmp = dir.join(".marks.json.tmp");
    std::fs::write(&tmp, body + "\n")?;
    std::fs::rename(tmp, dir.join(MARKS))
}

/// The transcript of a meeting, new name first.
pub fn transcript(dir: &Path) -> Option<PathBuf> {
    [TRANSCRIPT, OLD_TRANSCRIPT]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0))
}

/// A meeting folder, or `None` when it is not one.
pub fn meeting(dir: &Path) -> Option<Recording> {
    let name = dir.file_name()?.to_str()?;
    let started = crate::time::unix(crate::time::parse_meeting_name(name)?);
    let files = chunks(dir);
    let mut length = 0.0;
    let mut bytes = 0;
    for c in &files {
        if let Some(w) = crate::media::read_wav(c) {
            length += w.secs();
        }
        bytes += std::fs::metadata(c).map(|m| m.len()).unwrap_or(0);
    }
    let t = transcript(dir);
    Some(Recording {
        kind: Kind::Meeting,
        path: dir.display().to_string(),
        started,
        length,
        chunks: files.len() as u32,
        marks: read_marks(dir).len() as u32,
        transcript: match (&t, files.is_empty()) {
            (Some(_), _) => Transcript::Ready,
            (None, false) => Transcript::Missing,
            (None, true) => Transcript::None,
        },
        transcript_path: t.map(|p| p.display().to_string()),
        bytes,
    })
}

/// A screen capture file, or `None` when it is not one.
pub fn screen(path: &Path) -> Option<Recording> {
    let name = path.file_name()?.to_str()?;
    let started = crate::time::unix(crate::time::parse_screen_name(name)?);
    let bytes = std::fs::metadata(path).ok()?.len();
    Some(Recording {
        kind: Kind::Screen,
        path: path.display().to_string(),
        started,
        length: crate::media::read_mp4(path).unwrap_or(0.0),
        bytes,
        ..Default::default()
    })
}

/// Every recording under the two folders, newest first, at most `limit`.
/// `skip` names what is being recorded right now.
pub fn scan(meetings: &Path, screens: &Path, limit: usize, skip: &[&str]) -> Vec<Recording> {
    let mut out = vec![];
    let names = |dir: &Path, want_dir: bool| -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return vec![];
        };
        let mut v: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() == want_dir))
            .map(|e| e.path())
            .filter(|p| !skip.contains(&p.to_string_lossy().as_ref()))
            .collect();
        // The names sort by time, so only the newest need reading.
        v.sort();
        v.reverse();
        v.truncate(limit);
        v
    };
    let m = names(meetings, true);
    let s = names(screens, false);
    for d in m {
        if let Some(r) = meeting(&d).filter(|r| r.chunks > 0) {
            out.push(r);
        }
    }
    for f in s {
        if let Some(r) = screen(&f) {
            out.push(r);
        }
    }
    out.sort_by(|a, b| b.started.total_cmp(&a.started));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("logbook-lib-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn wav(secs: u32) -> Vec<u8> {
        let mut f = crate::media::wav_header(16000, 1, 16, secs * 32000);
        f.extend(std::iter::repeat_n(0u8, (secs * 32000) as usize));
        f
    }

    #[test]
    fn lists_meetings_and_captures() {
        let root = dir("scan");
        let meetings = root.join("meetings");
        let screens = root.join("videos");
        let m1 = meetings.join("2026-11-10_14-30-00");
        let m2 = meetings.join("2026-11-11_10-02-00");
        for d in [&m1, &m2, &screens] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::create_dir_all(meetings.join("2026-11-11_11-00-00")).unwrap();
        std::fs::write(m1.join("chunk-001-143000.wav"), wav(3)).unwrap();
        std::fs::write(m1.join("transcript.txt"), "hello\n").unwrap();
        std::fs::write(m2.join("chunk-002-101700.wav"), wav(1)).unwrap();
        std::fs::write(m2.join("chunk-001-100200.wav"), wav(2)).unwrap();
        write_marks(&m2, &[31.5, 2.0]).unwrap();
        std::fs::write(screens.join("recording-20261111-091532.mp4"), b"x").unwrap();
        std::fs::write(screens.join("holiday.mp4"), b"x").unwrap();

        let all = scan(&meetings, &screens, 10, &[]);
        assert_eq!(all.len(), 3, "{all:#?}");
        assert_eq!(all[0].path, m2.display().to_string());
        assert_eq!(all[0].chunks, 2);
        assert_eq!(all[0].length, 3.0);
        assert_eq!(all[0].marks, 2);
        assert_eq!(all[0].transcript, Transcript::Missing);
        assert_eq!(all[1].kind, Kind::Screen);
        assert_eq!(all[2].transcript, Transcript::Ready);
        assert!(all[2]
            .transcript_path
            .as_deref()
            .unwrap()
            .ends_with("transcript.txt"));

        assert_eq!(read_marks(&m2), vec![2.0, 31.5]);
        let names: Vec<String> = chunks(&m2)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["chunk-001-100200.wav", "chunk-002-101700.wav"]);

        let m2s = m2.display().to_string();
        let skipped = scan(&meetings, &screens, 10, &[m2s.as_str()]);
        assert_eq!(skipped.len(), 2);
        assert_eq!(scan(&meetings, &screens, 1, &[]).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
