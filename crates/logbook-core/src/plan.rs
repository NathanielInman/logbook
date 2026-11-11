//! How a meeting becomes `transcript.md`: the chunks are cut at the marks,
//! each piece is transcribed on its own, and each mark becomes a heading
//! right where it was dropped.

use std::path::PathBuf;

/// A mark closer than this to a chunk's edge moves onto the edge, so no
/// piece is too short to transcribe.
const SNAP: f64 = 1.5;

/// One piece to transcribe.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub chunk: PathBuf,
    /// Seconds into the chunk.
    pub from: f64,
    pub to: f64,
    /// Seconds into the meeting where it starts.
    pub at: f64,
    /// The marks (1-based number, seconds into the meeting) that open it.
    pub marks: Vec<(usize, f64)>,
    /// The whole chunk: no need to cut a copy.
    pub whole: bool,
}

impl Segment {
    pub fn secs(&self) -> f64 {
        (self.to - self.from).max(0.0)
    }
}

/// The plan for `chunks` (path and length in seconds, in order) and `marks`
/// (seconds into the meeting). Marks past the end come back separately:
/// they open no audio, but still get a heading.
pub fn plan(chunks: &[(PathBuf, f64)], marks: &[f64]) -> (Vec<Segment>, Vec<(usize, f64)>) {
    let mut marks: Vec<(usize, f64)> = {
        let mut m: Vec<f64> = marks.iter().copied().filter(|t| t.is_finite()).collect();
        m.sort_by(f64::total_cmp);
        m.into_iter()
            .enumerate()
            .map(|(i, t)| (i + 1, t.max(0.0)))
            .collect()
    };
    marks.reverse();
    let mut out = vec![];
    let mut offset = 0.0;
    for (path, len) in chunks {
        let len = len.max(0.0);
        let end = offset + len;
        // Cut points inside this chunk, snapped to its edges.
        let mut cuts: Vec<(f64, Vec<(usize, f64)>)> = vec![(0.0, vec![])];
        while let Some(&(n, t)) = marks.last() {
            if t >= end - SNAP && t < end && len > 0.0 {
                // Snaps to the end: it opens the next chunk instead.
                break;
            }
            if t >= end {
                break;
            }
            marks.pop();
            let local = (t - offset).max(0.0);
            if local < SNAP {
                cuts[0].1.push((n, t));
            } else if let Some(last) = cuts.last_mut().filter(|c| local - c.0 < SNAP) {
                last.1.push((n, t));
            } else {
                cuts.push((local, vec![(n, t)]));
            }
        }
        for (i, (from, opens)) in cuts.iter().enumerate() {
            let to = cuts.get(i + 1).map(|c| c.0).unwrap_or(len);
            out.push(Segment {
                chunk: path.clone(),
                from: *from,
                to,
                at: offset + from,
                marks: opens.clone(),
                whole: cuts.len() == 1,
            });
        }
        offset = end;
    }
    marks.reverse();
    (out, marks)
}

/// A finished piece: the marks that open it and what was said.
pub struct Part {
    pub marks: Vec<(usize, f64)>,
    pub text: String,
}

/// The transcript as Markdown.
pub fn render(title: &str, meta: &str, parts: &[Part]) -> String {
    let mut out = format!("# {title}\n\n{meta}\n");
    let mut prose = String::new();
    let flush = |out: &mut String, prose: &mut String| {
        if !prose.trim().is_empty() {
            out.push('\n');
            out.push_str(prose.trim());
            out.push('\n');
        }
        prose.clear();
    };
    for p in parts {
        if !p.marks.is_empty() {
            flush(&mut out, &mut prose);
            for (n, at) in &p.marks {
                out.push_str(&format!(
                    "\n## Mark {n} \u{b7} {}\n",
                    crate::time::clock(*at)
                ));
            }
        }
        let text = tidy(&p.text);
        if !text.is_empty() {
            if !prose.is_empty() {
                prose.push_str("\n\n");
            }
            prose.push_str(&text);
        }
    }
    flush(&mut out, &mut prose);
    out
}

/// One paragraph per line the transcriber printed, without its blank lines
/// or the `[BLANK_AUDIO]` whisper prints for silence.
pub fn tidy(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .map(|l| {
            l.replace("[BLANK_AUDIO]", "")
                .replace("[ Silence ]", "")
                .trim()
                .to_string()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: u32, len: f64) -> (PathBuf, f64) {
        (PathBuf::from(format!("chunk-{n:03}.wav")), len)
    }

    #[test]
    fn no_marks_is_one_piece_per_chunk() {
        let (s, rest) = plan(&[c(1, 900.0), c(2, 300.0)], &[]);
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|x| x.whole && x.marks.is_empty()));
        assert_eq!(s[1].at, 900.0);
        assert!(rest.is_empty());
    }

    #[test]
    fn marks_cut_the_chunk_they_fall_in() {
        let (s, rest) = plan(&[c(1, 900.0), c(2, 900.0)], &[972.0, 120.0]);
        assert_eq!(s.len(), 4);
        assert_eq!((s[0].from, s[0].to), (0.0, 120.0));
        assert!(!s[0].whole);
        assert_eq!(s[1].marks, vec![(1, 120.0)]);
        assert_eq!((s[1].from, s[1].to, s[1].at), (120.0, 900.0, 120.0));
        assert_eq!(s[2].chunk, PathBuf::from("chunk-002.wav"));
        assert_eq!((s[2].from, s[2].to), (0.0, 72.0));
        assert_eq!(s[3].marks, vec![(2, 972.0)]);
        assert_eq!(s[3].at, 972.0);
        assert!(rest.is_empty());
    }

    #[test]
    fn marks_near_an_edge_snap_to_it() {
        let (s, _) = plan(&[c(1, 900.0), c(2, 900.0)], &[0.4, 899.5, 1000.0, 1000.5]);
        assert_eq!(s[0].marks, vec![(1, 0.4)]);
        assert_eq!(s[0].to, 900.0);
        assert_eq!(s[1].marks, vec![(2, 899.5)]);
        assert_eq!(s[1].from, 0.0);
        assert_eq!(s[2].marks, vec![(3, 1000.0), (4, 1000.5)]);
    }

    #[test]
    fn a_mark_after_the_last_word() {
        let (s, rest) = plan(&[c(1, 60.0)], &[300.0]);
        assert_eq!(s.len(), 1);
        assert_eq!(rest, vec![(1, 300.0)]);
    }

    #[test]
    fn renders_markdown() {
        let parts = [
            Part {
                marks: vec![],
                text: " Morning everyone.\n[BLANK_AUDIO]\nLet's start.\n".into(),
            },
            Part {
                marks: vec![],
                text: "Second chunk.".into(),
            },
            Part {
                marks: vec![(1, 1872.0)],
                text: "We ship Friday.".into(),
            },
            Part {
                marks: vec![(2, 1900.0)],
                text: String::new(),
            },
        ];
        let md = render(
            "Meeting \u{b7} today 10:02",
            "52 min \u{b7} 2 marks",
            &parts,
        );
        assert_eq!(
            md,
            "# Meeting \u{b7} today 10:02\n\n52 min \u{b7} 2 marks\n\nMorning everyone.\n\nLet's start.\n\nSecond chunk.\n\n## Mark 1 \u{b7} 00:31:12\n\nWe ship Friday.\n\n## Mark 2 \u{b7} 00:31:40\n"
        );
    }
}
