//! The waybar line: one lamp for the mic. Dim while nothing listens, red
//! with a running clock while Logbook records, red and pulsing while
//! dictating, amber while a capture is saved or transcripts are made, and
//! its own color while another app holds the mic. The first class is the
//! state, so the bar's CSS colors it.

use logbook_core::config::Config;
use logbook_core::model::{Dictation, JobState, Lamp, ScreenPhase, Snapshot};
use logbook_core::protocol::BarLine;
use logbook_core::text::escape;
use logbook_core::time::clock;

fn text(icon: &str, words: &str) -> String {
    match (icon.is_empty(), words.is_empty()) {
        (_, true) => icon.to_string(),
        (true, false) => escape(words),
        (false, false) => format!("{icon} {}", escape(words)),
    }
}

/// Names the apps holding the mic: `chrome`, `chrome, zoom`.
fn others(s: &Snapshot) -> String {
    let mut v: Vec<&str> = s.others().map(|u| u.name.as_str()).collect();
    v.dedup();
    v.join(", ")
}

fn tooltip(s: &Snapshot) -> String {
    let mut t: Vec<String> = vec![];
    if let Some(m) = &s.meeting {
        let marks = match m.marks.len() {
            0 => String::new(),
            1 => " \u{b7} 1 mark".into(),
            n => format!(" \u{b7} {n} marks"),
        };
        t.push(format!(
            "Recording a meeting \u{b7} {} \u{b7} chunk {}{marks}",
            clock(s.now - m.started),
            m.chunk
        ));
    }
    if let Some(x) = &s.screen {
        t.push(match x.phase {
            ScreenPhase::Recording => {
                format!("Recording the screen \u{b7} {}", clock(s.now - x.started))
            }
            ScreenPhase::Processing => "Saving the screen capture (loudness pass)".into(),
        });
    }
    match s.dictation {
        Dictation::Recording => t.push("Dictating: release the key to type it".into()),
        Dictation::Transcribing => t.push("voxtype is typing what you said".into()),
        _ => {}
    }
    if let Some(j) = s.running_job() {
        let name = std::path::Path::new(&j.dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut line = format!("Transcribing {name}");
        if j.total > 0 {
            line.push_str(&format!(
                " \u{b7} {}/{}",
                (j.done + 1).min(j.total),
                j.total
            ));
        }
        if let Some(b) = &j.behind {
            line.push_str(&format!(" \u{b7} waiting for {b}"));
        }
        t.push(line);
    }
    let queued = s
        .jobs
        .iter()
        .filter(|j| j.state == JobState::Queued)
        .count();
    if queued > 0 {
        t.push(format!(
            "{queued} meeting{} queued for a transcript",
            if queued == 1 { "" } else { "s" }
        ));
    }
    for u in s.others() {
        t.push(format!("{} has the mic open ({})", u.name, u.source));
    }
    if t.is_empty() {
        t.push(if s.mic_known {
            "Mic closed. No app is listening.".into()
        } else {
            "Mic closed as far as Logbook knows (PipeWire is not answering)".into()
        });
    }
    escape(&t.join("\n"))
}

pub fn line(s: &Snapshot, cfg: &Config) -> BarLine {
    let b = &cfg.bar;
    let lamp = s.lamp;
    let (txt, mut class) = match lamp {
        Lamp::Recording => {
            let since = s
                .meeting
                .as_ref()
                .map(|m| m.started)
                .or_else(|| s.screen.as_ref().map(|x| x.started))
                .unwrap_or(s.now);
            let mut class = vec!["recording".to_string()];
            if s.meeting.is_some() {
                class.push("meeting".into());
            }
            if s.screen
                .as_ref()
                .is_some_and(|x| x.phase == ScreenPhase::Recording)
            {
                class.push("screen".into());
            }
            (text(&b.icon_rec, &clock(s.now - since)), class)
        }
        Lamp::Dictating => (text(&b.icon_dictate, "listening"), vec!["dictating".into()]),
        Lamp::Processing => {
            let w = if s.screen.is_some() {
                "saving"
            } else {
                "typing"
            };
            (text(&b.icon_busy, w), vec!["processing".into()])
        }
        Lamp::Transcribing => {
            let w = match s.running_job() {
                Some(j) if j.behind.is_some() => "queued".to_string(),
                Some(j) if j.total > 0 => format!("{}/{}", (j.done + 1).min(j.total), j.total),
                _ => "queued".into(),
            };
            (text(&b.icon_busy, &w), vec!["transcribing".into()])
        }
        Lamp::Mic => {
            let who = if b.show_app { others(s) } else { String::new() };
            (text(&b.icon_mic, &who), vec!["mic".into()])
        }
        Lamp::Off => (b.icon_off.clone(), vec!["off".into()]),
    };
    // Something else holds the mic while Logbook records: say so in CSS.
    if lamp != Lamp::Mic && s.others().next().is_some() {
        class.push("shared".into());
    }
    BarLine {
        text: txt,
        alt: lamp.as_str().into(),
        class,
        tooltip: tooltip(s),
    }
}

/// What the bar shows while the daemon cannot be reached.
pub fn offline() -> BarLine {
    BarLine {
        text: Config::default().bar.icon_off,
        class: vec!["offline".into()],
        alt: "offline".into(),
        tooltip: "The Logbook daemon is not running".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logbook_core::model::{lamp, Job, Meeting, MicUser, Screen};

    fn snap() -> Snapshot {
        Snapshot {
            now: 10_000.0,
            mic_known: true,
            ..Default::default()
        }
    }

    fn with_lamp(mut s: Snapshot) -> Snapshot {
        s.lamp = lamp(&s);
        s
    }

    #[test]
    fn off() {
        let l = line(&with_lamp(snap()), &Config::default());
        assert_eq!(l.class, ["off"]);
        assert_eq!(l.text, Config::default().bar.icon_off);
        assert_eq!(l.tooltip, "Mic closed. No app is listening.");
    }

    #[test]
    fn a_meeting_shows_its_clock() {
        let mut s = snap();
        s.meeting = Some(Meeting {
            started: 10_000.0 - (41.0 * 60.0 + 12.0),
            chunk: 3,
            marks: vec![1.0, 2.0],
            ..Default::default()
        });
        s.mic.push(MicUser {
            name: "chrome".into(),
            source: "Shure MV7".into(),
            ..Default::default()
        });
        let mut cfg = Config::default();
        cfg.bar.icon_rec = "R".into();
        let l = line(&with_lamp(s), &cfg);
        assert_eq!(l.text, "R 00:41:12");
        assert_eq!(l.class, ["recording", "meeting", "shared"]);
        assert!(l.tooltip.contains("chunk 3 \u{b7} 2 marks"));
        assert!(l.tooltip.contains("chrome has the mic open (Shure MV7)"));
    }

    #[test]
    fn another_app_on_the_mic() {
        let mut s = snap();
        s.mic.push(MicUser {
            name: "zoom".into(),
            ..Default::default()
        });
        let mut cfg = Config::default();
        cfg.bar.icon_mic = "M".into();
        let l = line(&with_lamp(s.clone()), &cfg);
        assert_eq!((l.text.as_str(), l.alt.as_str()), ("M zoom", "mic"));
        cfg.bar.show_app = false;
        assert_eq!(line(&with_lamp(s), &cfg).text, "M");
    }

    #[test]
    fn transcribing_and_saving() {
        let mut s = snap();
        s.jobs.push(Job {
            state: logbook_core::model::JobState::Running,
            done: 1,
            total: 3,
            ..Default::default()
        });
        let mut cfg = Config::default();
        cfg.bar.icon_busy = "B".into();
        assert_eq!(line(&with_lamp(s.clone()), &cfg).text, "B 2/3");
        s.jobs[0].behind = Some("det33 build".into());
        let l = line(&with_lamp(s.clone()), &cfg);
        assert_eq!(l.text, "B queued");
        assert!(l.tooltip.contains("waiting for det33 build"));
        s.screen = Some(Screen {
            phase: ScreenPhase::Processing,
            ..Default::default()
        });
        assert_eq!(line(&with_lamp(s), &cfg).text, "B saving");
    }

    #[test]
    fn markup_is_escaped() {
        let mut s = snap();
        s.mic.push(MicUser {
            name: "a<b>".into(),
            ..Default::default()
        });
        let l = line(&with_lamp(s), &Config::default());
        assert!(l.text.ends_with("a&lt;b&gt;"));
    }
}
