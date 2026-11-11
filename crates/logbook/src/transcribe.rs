//! `logbook transcribe <dir>`: turns a meeting folder into `transcript.md`.
//! Switchyard runs it as a heavy task, or the daemon runs it under
//! `lowprio`; either way it reports its progress to the daemon, and it works
//! the same from a terminal with no daemon at all.

use anyhow::{anyhow, Context, Result};
use logbook_core::config::Config;
use logbook_core::plan::{self, Part};
use logbook_core::protocol::Request;
use logbook_core::{library, media, paths, time};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn report(req: Request) {
    let _ = logbook_core::protocol::request_within(&req, Duration::from_secs(2));
}

/// Runs the transcriber on one file and returns what it printed.
fn transcribe_file(cmd: &[String], file: &Path) -> Result<String> {
    let (prog, args) = cmd
        .split_first()
        .ok_or_else(|| anyhow!("transcribe.command is empty"))?;
    let out = Command::new(prog)
        .args(args)
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("cannot run {prog}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{prog} exited with {} on {}",
            out.status,
            file.display()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn run(dir: &str) -> Result<i32> {
    let dir = paths::expand(dir);
    let dir = dir.canonicalize().unwrap_or(dir);
    let key = dir.display().to_string();
    match work(&dir, &key) {
        Ok(path) => {
            eprintln!("logbook: transcript written to {}", path.display());
            report(Request::Finished {
                dir: key,
                ok: true,
                error: None,
            });
            Ok(0)
        }
        Err(e) => {
            report(Request::Finished {
                dir: key,
                ok: false,
                error: Some(format!("{e:#}")),
            });
            Err(e)
        }
    }
}

fn work(dir: &Path, key: &str) -> Result<PathBuf> {
    let (cfg, _) = Config::load();
    let chunks: Vec<(PathBuf, f64)> = library::chunks(dir)
        .into_iter()
        .filter_map(|p| {
            let len = media::read_wav(&p)?.secs();
            Some((p, len))
        })
        .collect();
    if chunks.is_empty() {
        return Err(anyhow!("no chunk-*.wav files in {}", dir.display()));
    }
    let marks = library::read_marks(dir);
    let (segments, trailing) = plan::plan(&chunks, &marks);
    let total = segments.len() as u32;
    let total_secs: f64 = segments.iter().map(|s| s.secs()).sum();
    let mut done_secs = 0.0;
    let tmp = std::env::temp_dir().join(format!("logbook-cut-{}.wav", std::process::id()));
    let mut parts = vec![];
    for (i, seg) in segments.iter().enumerate() {
        report(Request::Progress {
            dir: key.to_string(),
            done: i as u32,
            total,
            done_secs,
            total_secs,
        });
        let name = seg
            .chunk
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!(
            "===== {}/{total}: {name} {}-{} =====",
            i + 1,
            time::clock(seg.from),
            time::clock(seg.to)
        );
        let text = if seg.secs() < 0.5 {
            String::new()
        } else if seg.whole {
            transcribe_file(&cfg.transcribe.command, &seg.chunk)?
        } else {
            media::slice_wav(&seg.chunk, seg.from, seg.to, &tmp)?;
            let r = transcribe_file(&cfg.transcribe.command, &tmp);
            let _ = std::fs::remove_file(&tmp);
            r?
        };
        done_secs += seg.secs();
        parts.push(Part {
            marks: seg.marks.clone(),
            text,
        });
    }
    if !trailing.is_empty() {
        parts.push(Part {
            marks: trailing,
            text: String::new(),
        });
    }
    report(Request::Progress {
        dir: key.to_string(),
        done: total,
        total,
        done_secs: total_secs,
        total_secs,
    });

    let started = dir
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(time::parse_meeting_name)
        .map(time::unix);
    let title = match started {
        Some(t) => format!("Meeting \u{b7} {}", time::long(t)),
        None => format!(
            "Meeting \u{b7} {}",
            dir.file_name().unwrap_or_default().to_string_lossy()
        ),
    };
    let mut meta = vec![
        time::length(total_secs),
        format!(
            "{} chunk{}",
            chunks.len(),
            if chunks.len() == 1 { "" } else { "s" }
        ),
    ];
    if !marks.is_empty() {
        meta.push(format!(
            "{} mark{}",
            marks.len(),
            if marks.len() == 1 { "" } else { "s" }
        ));
    }
    if let Some(p) = cfg.transcribe.command.first() {
        meta.push(
            Path::new(p)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        );
    }
    let md = plan::render(&title, &meta.join(" \u{b7} "), &parts);
    let out = dir.join(library::TRANSCRIPT);
    let part = dir.join(".transcript.md.part");
    std::fs::write(&part, md)?;
    std::fs::rename(&part, &out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcribes_a_meeting_with_a_fake_transcriber() {
        let root = std::env::temp_dir().join(format!("logbook-tx-{}", std::process::id()));
        let dir = root.join("2026-11-11_10-02-00");
        std::fs::create_dir_all(&dir).unwrap();
        let wav = |secs: u32| {
            let mut f = media::wav_header(16000, 1, 16, secs * 32000);
            f.extend(std::iter::repeat_n(0u8, (secs * 32000) as usize));
            f
        };
        std::fs::write(dir.join("chunk-001-100200.wav"), wav(20)).unwrap();
        std::fs::write(dir.join("chunk-002-100220.wav"), wav(10)).unwrap();
        library::write_marks(&dir, &[25.0]).unwrap();
        // A transcriber that says how long each piece is.
        let fake = root.join("fake.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\nsz=$(stat -c %s \"$1\")\necho \"[BLANK_AUDIO]\"\necho \"piece of $(( (sz - 44) / 32000 )) seconds\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfgdir = root.join("cfg");
        std::fs::create_dir_all(&cfgdir).unwrap();
        std::fs::write(
            cfgdir.join("config.toml"),
            format!("[transcribe]\ncommand = [\"{}\"]\n", fake.display()),
        )
        .unwrap();
        std::env::set_var("LOGBOOK_CONFIG_DIR", &cfgdir);
        std::env::set_var("LOGBOOK_RUNTIME_DIR", root.join("run"));
        let out = work(&dir, "x").unwrap();
        let md = std::fs::read_to_string(out).unwrap();
        assert!(md.starts_with("# Meeting \u{b7} Wed 11 Nov 2026 10:02\n\n0:30 \u{b7} 2 chunks \u{b7} 1 mark \u{b7} fake.sh\n"), "{md}");
        assert!(md.contains("piece of 20 seconds\n\npiece of 5 seconds\n\n## Mark 1 \u{b7} 00:00:25\n\npiece of 5 seconds\n"), "{md}");
        assert!(!md.contains("BLANK"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
