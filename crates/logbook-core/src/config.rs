//! `~/.config/logbook/config.toml`. Every key is optional; an empty or
//! missing file gives a working setup.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bar: BarConfig,
    pub meeting: MeetingConfig,
    pub screen: ScreenConfig,
    pub transcribe: TranscribeConfig,
    pub mic: MicConfig,
    pub drawer: DrawerConfig,
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BarConfig {
    /// The mic glyph while nothing listens.
    pub icon_off: String,
    /// While another app has the mic open.
    pub icon_mic: String,
    /// While Logbook records a meeting or the screen.
    pub icon_rec: String,
    /// While dictating.
    pub icon_dictate: String,
    /// While a screen capture is processed or transcripts are made.
    pub icon_busy: String,
    /// Name the app that has the mic open next to the glyph.
    pub show_app: bool,
    /// The layer-shell namespace of the bar surface, used to find its edge.
    pub namespace: String,
}

impl Default for BarConfig {
    fn default() -> Self {
        BarConfig {
            icon_off: "\u{f036c}".into(),
            icon_mic: "\u{f036c}".into(),
            icon_rec: "\u{f044a}".into(),
            icon_dictate: "\u{f036c}".into(),
            icon_busy: "\u{f051f}".into(),
            show_app: true,
            namespace: "waybar".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MeetingConfig {
    /// One folder per meeting goes here, named by its start time.
    pub dir: String,
    /// Seconds per chunk file.
    pub chunk: u64,
    /// pw-record's `--target`; empty records the default input.
    pub source: String,
    /// Turn on swaync's do not disturb while recording, and restore it after.
    pub dnd: bool,
    /// Queue a transcript when a meeting stops.
    pub transcribe: bool,
}

impl Default for MeetingConfig {
    fn default() -> Self {
        MeetingConfig {
            dir: "~/Audio/meetings".into(),
            chunk: 900,
            source: String::new(),
            dnd: true,
            transcribe: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ScreenConfig {
    /// Captures go here as `recording-YYYYmmdd-HHMMSS.mp4`.
    pub dir: String,
    /// `monitor` records what the default output plays, `none` records no
    /// sound, anything else is a PipeWire source name.
    pub audio: String,
    /// Normalize the loudness after a capture (ffmpeg loudnorm, video
    /// copied as is).
    pub loudnorm: bool,
    pub dnd: bool,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        ScreenConfig {
            dir: "~/Videos".into(),
            audio: "monitor".into(),
            loudnorm: true,
            dnd: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TranscribeConfig {
    /// `auto` queues in Switchyard when it runs and has the task, else runs
    /// under `wrap`; `switchyard` or `direct` force one.
    pub runner: String,
    /// The Switchyard task `logbook setup` installs.
    pub task: String,
    /// The transcriber; the audio file is appended.
    pub command: Vec<String>,
    /// What a direct run is wrapped in, so it never fights a build for the
    /// machine. Empty runs it bare.
    pub wrap: Vec<String>,
}

impl Default for TranscribeConfig {
    fn default() -> Self {
        TranscribeConfig {
            runner: "auto".into(),
            task: "logbook-transcribe".into(),
            command: vec!["voxtype".into(), "transcribe".into()],
            wrap: vec!["lowprio".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MicConfig {
    /// Light the lamp while another app has the mic open.
    pub watch: bool,
    /// Programs (binary or application name, any case) never shown as
    /// holding the mic.
    pub ignore: Vec<String>,
}

impl Default for MicConfig {
    fn default() -> Self {
        MicConfig {
            watch: true,
            ignore: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DrawerConfig {
    /// Panel width in logical pixels.
    pub width: f64,
    /// Largest height as a fraction of the monitor.
    pub max_height: f64,
    /// Gap between the bar and the panel.
    pub gap: f64,
    /// How many recordings the lists show.
    pub recent: usize,
}

impl Default for DrawerConfig {
    fn default() -> Self {
        DrawerConfig {
            width: 440.0,
            max_height: 0.8,
            gap: 8.0,
            recent: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// `waybar` takes the font and colors from waybar's style.css; `none`
    /// keeps the built-in theme.
    pub from: String,
    pub accent: Option<String>,
    pub font: Option<String>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        ThemeConfig {
            from: "waybar".into(),
            accent: None,
            font: None,
        }
    }
}

impl Config {
    /// Reads the config file. A broken file gives the defaults and the
    /// error, so the daemon keeps running and can say what is wrong.
    pub fn load() -> (Config, Option<String>) {
        let path = crate::paths::config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => match Config::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(e)),
            },
            Err(_) => (Config::default(), None),
        }
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        toml::from_str::<Config>(text).map_err(|e| {
            format!(
                "{}: {}",
                crate::paths::tilde(&crate::paths::config_file()),
                e
            )
        })
    }

    pub fn meeting_dir(&self) -> std::path::PathBuf {
        crate::paths::expand(&self.meeting.dir)
    }

    pub fn screen_dir(&self) -> std::path::PathBuf {
        crate::paths::expand(&self.screen.dir)
    }

    /// Whether a mic user is on the ignore list.
    pub fn ignored(&self, binary: &str, app: &str) -> bool {
        self.mic.ignore.iter().any(|i| {
            let i = i.trim();
            !i.is_empty() && (i.eq_ignore_ascii_case(binary) || i.eq_ignore_ascii_case(app))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_the_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn reads_keys_and_rejects_typos() {
        let c = Config::parse(
            "[meeting]\nchunk = 600\ndnd = false\n[mic]\nignore = [\"obs\"]\n[transcribe]\nrunner = \"direct\"\n",
        )
        .unwrap();
        assert_eq!(c.meeting.chunk, 600);
        assert!(!c.meeting.dnd);
        assert_eq!(c.transcribe.runner, "direct");
        assert!(c.ignored("OBS", ""));
        assert!(!c.ignored("chrome", "Google Chrome input"));
        assert!(Config::parse("[meeting]\nchunks = 600\n").is_err());
    }
}
