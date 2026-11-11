//! What the daemon knows, as one snapshot the bar module, the drawer and
//! `logbook status` all read.

use serde::{Deserialize, Serialize};

/// The lamp's state, most urgent first. The bar shows the first that holds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lamp {
    /// Logbook records a meeting or the screen.
    Recording,
    /// voxtype is listening.
    Dictating,
    /// A screen capture's loudness pass, or voxtype typing out.
    Processing,
    /// Transcripts are being made or wait in the queue.
    Transcribing,
    /// Another app has the mic open.
    Mic,
    /// Nothing listens.
    #[default]
    Off,
}

impl Lamp {
    pub fn as_str(self) -> &'static str {
        match self {
            Lamp::Recording => "recording",
            Lamp::Dictating => "dictating",
            Lamp::Processing => "processing",
            Lamp::Transcribing => "transcribing",
            Lamp::Mic => "mic",
            Lamp::Off => "off",
        }
    }
}

/// voxtype's daemon, from its state file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Dictation {
    /// Not running (or not installed).
    #[default]
    Stopped,
    Idle,
    Recording,
    Transcribing,
}

impl Dictation {
    pub fn as_str(self) -> &'static str {
        match self {
            Dictation::Stopped => "stopped",
            Dictation::Idle => "idle",
            Dictation::Recording => "recording",
            Dictation::Transcribing => "transcribing",
        }
    }
}

/// A meeting being recorded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Meeting {
    pub dir: String,
    /// Unix seconds.
    pub started: f64,
    /// 1-based number of the chunk being written.
    pub chunk: u32,
    /// The chunk file being written, for the drawer's level meter.
    pub chunk_path: String,
    /// Seconds from the start of each mark so far.
    pub marks: Vec<f64>,
    /// The input being recorded.
    pub source: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScreenPhase {
    #[default]
    Recording,
    /// The loudness pass after stop.
    Processing,
}

/// A screen capture being recorded or processed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Screen {
    pub path: String,
    pub started: f64,
    pub phase: ScreenPhase,
    /// The region, as slurp printed it.
    pub geometry: String,
    /// When the loudness pass began.
    #[serde(default)]
    pub processing_since: Option<f64>,
}

/// An app with a capture stream on a microphone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct MicUser {
    /// What to call it: the binary (`chrome`, `zoom`), else the app name.
    pub name: String,
    pub app: String,
    pub binary: String,
    pub pid: Option<u32>,
    /// The microphone it records from.
    pub source: String,
    /// One of Logbook's own recorders.
    pub ours: bool,
    /// voxtype dictating.
    pub dictation: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    #[default]
    Queued,
    Running,
    Failed,
}

/// A meeting waiting for, or getting, its transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Job {
    pub dir: String,
    pub state: JobState,
    /// `switchyard` or `direct`, once started.
    pub runner: Option<String>,
    /// Segments done and in all.
    pub done: u32,
    pub total: u32,
    /// Audio seconds done and in all, for the estimate.
    pub done_secs: f64,
    pub total_secs: f64,
    pub started: Option<f64>,
    /// What Switchyard runs first: a heavy task ahead in its queue, or why
    /// it holds the main line.
    pub behind: Option<String>,
    pub error: Option<String>,
}

impl Job {
    /// Seconds left, once there is enough to go on.
    pub fn eta(&self, now: f64) -> Option<f64> {
        let started = self.started?;
        if self.done_secs < 1.0 || self.total_secs <= self.done_secs {
            return None;
        }
        let spent = (now - started).max(0.0);
        Some(spent / self.done_secs * (self.total_secs - self.done_secs))
    }

    /// 0 to 1.
    pub fn fraction(&self) -> f64 {
        if self.total_secs > 0.0 {
            (self.done_secs / self.total_secs).clamp(0.0, 1.0)
        } else if self.total > 0 {
            (self.done as f64 / self.total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Meeting,
    Screen,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transcript {
    /// Not a meeting, or nothing to transcribe.
    #[default]
    None,
    /// A meeting with no transcript yet.
    Missing,
    Queued,
    Running,
    Failed,
    Ready,
}

/// A finished recording on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Recording {
    pub kind: Kind,
    /// The meeting folder or the capture file.
    pub path: String,
    /// Unix seconds, from the name.
    pub started: f64,
    /// Seconds of audio or video.
    pub length: f64,
    pub chunks: u32,
    pub marks: u32,
    pub transcript: Transcript,
    /// `transcript.md`, or the old scripts' `transcript.txt`.
    pub transcript_path: Option<String>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Snapshot {
    pub version: String,
    /// Unix seconds on the daemon's clock.
    pub now: f64,
    pub lamp: Lamp,
    pub meeting: Option<Meeting>,
    pub screen: Option<Screen>,
    pub dictation: Dictation,
    /// Every app holding a microphone, Logbook's own included.
    pub mic: Vec<MicUser>,
    /// PipeWire answered; without it the mic list is unknown.
    pub mic_known: bool,
    pub jobs: Vec<Job>,
    pub recordings: Vec<Recording>,
    /// Logbook turned do not disturb on and will turn it off again.
    pub dnd_held: bool,
    pub config_error: Option<String>,
    /// The last thing that went wrong, for the drawer.
    pub problem: Option<String>,
}

impl Snapshot {
    /// The apps other than Logbook and dictation that hold the mic.
    pub fn others(&self) -> impl Iterator<Item = &MicUser> {
        self.mic.iter().filter(|u| !u.ours && !u.dictation)
    }

    pub fn running_job(&self) -> Option<&Job> {
        self.jobs.iter().find(|j| j.state == JobState::Running)
    }

    pub fn pending_jobs(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state != JobState::Failed)
            .count()
    }
}

/// Which lamp a set of facts lights.
pub fn lamp(s: &Snapshot) -> Lamp {
    if s.meeting.is_some()
        || s.screen
            .as_ref()
            .is_some_and(|x| x.phase == ScreenPhase::Recording)
    {
        Lamp::Recording
    } else if s.dictation == Dictation::Recording {
        Lamp::Dictating
    } else if s.screen.is_some() || s.dictation == Dictation::Transcribing {
        Lamp::Processing
    } else if s.pending_jobs() > 0 {
        Lamp::Transcribing
    } else if s.others().next().is_some() {
        Lamp::Mic
    } else {
        Lamp::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamp_order() {
        let mut s = Snapshot::default();
        assert_eq!(lamp(&s), Lamp::Off);
        s.mic.push(MicUser {
            name: "chrome".into(),
            ..Default::default()
        });
        assert_eq!(lamp(&s), Lamp::Mic);
        s.jobs.push(Job::default());
        assert_eq!(lamp(&s), Lamp::Transcribing);
        s.dictation = Dictation::Transcribing;
        assert_eq!(lamp(&s), Lamp::Processing);
        s.dictation = Dictation::Recording;
        assert_eq!(lamp(&s), Lamp::Dictating);
        s.meeting = Some(Meeting::default());
        assert_eq!(lamp(&s), Lamp::Recording);
    }

    #[test]
    fn our_own_streams_do_not_light_the_mic_lamp() {
        let mut s = Snapshot::default();
        s.mic.push(MicUser {
            name: "pw-record".into(),
            ours: true,
            ..Default::default()
        });
        s.mic.push(MicUser {
            name: "voxtype".into(),
            dictation: true,
            ..Default::default()
        });
        assert_eq!(lamp(&s), Lamp::Off);
    }

    #[test]
    fn failed_jobs_do_not_keep_the_lamp_amber() {
        let mut s = Snapshot::default();
        s.jobs.push(Job {
            state: JobState::Failed,
            ..Default::default()
        });
        assert_eq!(lamp(&s), Lamp::Off);
    }

    #[test]
    fn estimate() {
        let j = Job {
            started: Some(100.0),
            done_secs: 300.0,
            total_secs: 900.0,
            ..Default::default()
        };
        assert_eq!(j.eta(160.0), Some(120.0));
        assert!((j.fraction() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(Job::default().eta(10.0), None);
    }
}
