//! Everything Logbook knows that is not a process, PipeWire or drawing:
//! where files live, the config, the model, the socket protocol, the
//! recordings on disk, the transcript plan, and the parsers for WAV, MP4,
//! PipeWire's graph, voxtype's state and Switchyard's snapshot.

pub mod config;
pub mod library;
pub mod media;
pub mod model;
pub mod paths;
pub mod protocol;
pub mod text;
pub mod time;
pub mod transport;
pub mod waybar;

pub use bar_anchor::{Anchor, Edge};

/// Unix time in seconds.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
