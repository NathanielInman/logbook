//! Just enough of WAV and MP4 to know how long a recording is, and to cut a
//! meeting chunk at a mark.

use anyhow::{anyhow, Result};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The format of a PCM WAV file and where its samples are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wav {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    /// Byte offset of the first sample.
    pub data_at: u64,
    /// Bytes of samples. pw-record writes the size when it stops; while it
    /// records (or after a crash) the header says 0 or too much, so this
    /// is taken from the file size instead.
    pub data_len: u64,
}

impl Wav {
    pub fn bytes_per_sec(&self) -> u64 {
        self.rate as u64 * self.channels as u64 * (self.bits as u64 / 8).max(1)
    }

    pub fn block(&self) -> u64 {
        self.channels as u64 * (self.bits as u64 / 8).max(1)
    }

    pub fn secs(&self) -> f64 {
        let bps = self.bytes_per_sec();
        if bps == 0 {
            0.0
        } else {
            self.data_len as f64 / bps as f64
        }
    }
}

/// Reads the header of a RIFF WAV, given the first bytes and the file size.
pub fn parse_wav(head: &[u8], file_len: u64) -> Option<Wav> {
    if head.len() < 12 || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_le_bytes(head.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(head.get(i..i + 4)?.try_into().ok()?));
    let mut at = 12usize;
    let mut fmt: Option<(u16, u32, u16)> = None;
    while at + 8 <= head.len() {
        let id = &head[at..at + 4];
        let size = u32_at(at + 4)? as u64;
        let body = at + 8;
        if id == b"fmt " {
            fmt = Some((u16_at(body + 2)?, u32_at(body + 4)?, u16_at(body + 14)?));
        } else if id == b"data" {
            let (channels, rate, bits) = fmt?;
            let data_at = body as u64;
            let on_disk = file_len.saturating_sub(data_at);
            let data_len = if size == 0 || size == u32::MAX as u64 || size > on_disk {
                on_disk
            } else {
                size
            };
            let w = Wav {
                rate,
                channels,
                bits,
                data_at,
                data_len,
            };
            let block = w.block();
            return Some(Wav {
                data_len: data_len - data_len % block.max(1),
                ..w
            });
        }
        at = body + size as usize + (size as usize & 1);
    }
    None
}

pub fn read_wav(path: &Path) -> Option<Wav> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = vec![0u8; 4096.min(len as usize)];
    f.read_exact(&mut head).ok()?;
    parse_wav(&head, len)
}

/// A canonical 44-byte PCM header.
pub fn wav_header(rate: u32, channels: u16, bits: u16, data_len: u32) -> Vec<u8> {
    let block = channels as u32 * (bits as u32 / 8);
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data_len).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * block).to_le_bytes());
    h.extend_from_slice(&(block as u16).to_le_bytes());
    h.extend_from_slice(&bits.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    h
}

/// Copies `from..to` seconds of a WAV into a new file with a clean header.
pub fn slice_wav(src: &Path, from: f64, to: f64, dst: &Path) -> Result<()> {
    let w = read_wav(src).ok_or_else(|| anyhow!("{} is not a WAV file", src.display()))?;
    let bps = w.bytes_per_sec() as f64;
    let block = w.block();
    let at = |t: f64| {
        let b = (t.max(0.0) * bps) as u64;
        (b - b % block).min(w.data_len)
    };
    let (a, b) = (at(from), at(to));
    let len = b.saturating_sub(a);
    let mut f = std::fs::File::open(src)?;
    f.seek(SeekFrom::Start(w.data_at + a))?;
    let mut out = std::fs::File::create(dst)?;
    out.write_all(&wav_header(w.rate, w.channels, w.bits, len as u32))?;
    std::io::copy(&mut f.take(len), &mut out)?;
    Ok(())
}

/// The loudness of the last `secs` of a 16-bit WAV as 0 to 1 (RMS against
/// full scale, on a gentle curve so speech fills the meter).
pub fn tail_level(path: &Path, secs: f64) -> Option<f64> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = vec![0u8; 4096.min(len as usize)];
    f.read_exact(&mut head).ok()?;
    let w = parse_wav(&head, len)?;
    if w.bits != 16 {
        return None;
    }
    let want = ((secs * w.bytes_per_sec() as f64) as u64).min(w.data_len);
    let want = want - want % 2;
    if want == 0 {
        return Some(0.0);
    }
    f.seek(SeekFrom::Start(w.data_at + w.data_len - want))
        .ok()?;
    let mut buf = vec![0u8; want as usize];
    f.read_exact(&mut buf).ok()?;
    Some(level(&buf))
}

/// RMS of little-endian 16-bit samples, on a curve.
pub fn level(pcm: &[u8]) -> f64 {
    let n = pcm.len() / 2;
    if n == 0 {
        return 0.0;
    }
    let sum: f64 = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            let v = i16::from_le_bytes(*c) as f64 / 32768.0;
            v * v
        })
        .sum();
    let rms = (sum / n as f64).sqrt();
    // -60 dBFS reads as silent, -6 dBFS as full.
    let db = 20.0 * rms.max(1e-9).log10();
    ((db + 60.0) / 54.0).clamp(0.0, 1.0)
}

/// The length of an MP4 from its `moov/mvhd` box.
pub fn parse_mp4(data: &[u8]) -> Option<f64> {
    let mvhd = find_box(data, &[b"moov", b"mvhd"])?;
    let version = *mvhd.first()?;
    let (scale, duration) = if version == 1 {
        let s = u32::from_be_bytes(mvhd.get(20..24)?.try_into().ok()?);
        let d = u64::from_be_bytes(mvhd.get(24..32)?.try_into().ok()?);
        (s, d)
    } else {
        let s = u32::from_be_bytes(mvhd.get(12..16)?.try_into().ok()?);
        let d = u32::from_be_bytes(mvhd.get(16..20)?.try_into().ok()?) as u64;
        (s, d)
    };
    (scale > 0).then(|| duration as f64 / scale as f64)
}

/// The body of the box at `path`, searching only the top level of each.
fn find_box<'a>(mut data: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    for want in path {
        let mut found = None;
        let mut at = 0usize;
        while at + 8 <= data.len() {
            let size32 = u32::from_be_bytes(data[at..at + 4].try_into().ok()?) as u64;
            let kind = &data[at + 4..at + 8];
            let (head, size) = match size32 {
                1 => (
                    16usize,
                    u64::from_be_bytes(data.get(at + 8..at + 16)?.try_into().ok()?),
                ),
                0 => (8usize, (data.len() - at) as u64),
                n => (8usize, n),
            };
            if size < head as u64 {
                return None;
            }
            let end = (at as u64 + size).min(data.len() as u64) as usize;
            if kind == *want {
                found = Some(&data[at + head..end]);
                break;
            }
            at = end;
        }
        data = found?;
    }
    Some(data)
}

/// Reads an MP4's length without loading the media: the `moov` box sits at
/// the start or the end, and only top-level headers are read until then.
pub fn read_mp4(path: &Path) -> Option<f64> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut h = [0u8; 16];
        let n = f.read(&mut h).ok()?;
        if n < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes(h[0..4].try_into().ok()?) as u64;
        let size = match size32 {
            1 if n >= 16 => u64::from_be_bytes(h[8..16].try_into().ok()?),
            0 => len - at,
            s => s,
        };
        if size < 8 {
            return None;
        }
        if &h[4..8] == b"moov" {
            if size > 64 << 20 {
                return None;
            }
            f.seek(SeekFrom::Start(at)).ok()?;
            let mut buf = vec![0u8; size as usize];
            f.read_exact(&mut buf).ok()?;
            return parse_mp4(&buf);
        }
        at += size;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("logbook-media-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn reads_a_finished_wav() {
        let mut f = wav_header(16000, 1, 16, 64000);
        f.extend(std::iter::repeat_n(0u8, 64000));
        let w = parse_wav(&f, f.len() as u64).unwrap();
        assert_eq!(w.rate, 16000);
        assert_eq!(w.data_at, 44);
        assert_eq!(w.secs(), 2.0);
    }

    #[test]
    fn reads_a_wav_still_being_written() {
        // pw-record leaves the sizes at 0 until it stops.
        let mut f = wav_header(16000, 1, 16, 0);
        f.extend(std::iter::repeat_n(0u8, 32001));
        let w = parse_wav(&f, f.len() as u64).unwrap();
        assert_eq!(w.data_len, 32000);
        assert_eq!(w.secs(), 1.0);
        assert!(parse_wav(b"RIFF....AVI ", 12).is_none());
    }

    #[test]
    fn slices_at_a_mark() {
        let src = tmp("src.wav");
        let mut f = wav_header(16000, 1, 16, 96000);
        f.extend((0..48000u32).flat_map(|i| (i as i16).to_le_bytes()));
        std::fs::write(&src, &f).unwrap();
        let dst = tmp("cut.wav");
        slice_wav(&src, 1.0, 2.5, &dst).unwrap();
        let w = read_wav(&dst).unwrap();
        assert_eq!(w.secs(), 1.5);
        let bytes = std::fs::read(&dst).unwrap();
        assert_eq!(i16::from_le_bytes([bytes[44], bytes[45]]), 16000);
        slice_wav(&src, 2.5, 99.0, &dst).unwrap();
        assert_eq!(read_wav(&dst).unwrap().secs(), 0.5);
    }

    #[test]
    fn levels() {
        assert_eq!(level(&[0, 0, 0, 0]), 0.0);
        let loud: Vec<u8> = (0..1000)
            .flat_map(|i| if i % 2 == 0 { 16000i16 } else { -16000 }.to_le_bytes())
            .collect();
        assert!(level(&loud) > 0.9);
    }

    fn mp4(version: u8, scale: u32, duration: u64) -> Vec<u8> {
        let mut mvhd = vec![version, 0, 0, 0];
        if version == 1 {
            mvhd.extend([0u8; 16]);
            mvhd.extend(scale.to_be_bytes());
            mvhd.extend(duration.to_be_bytes());
        } else {
            mvhd.extend([0u8; 8]);
            mvhd.extend(scale.to_be_bytes());
            mvhd.extend((duration as u32).to_be_bytes());
        }
        mvhd.extend([0u8; 80]);
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend(body);
            b
        };
        let mut out = boxed(b"ftyp", b"isom\0\0\x02\0");
        out.extend(boxed(b"mdat", &[0u8; 100]));
        out.extend(boxed(b"moov", &boxed(b"mvhd", &mvhd)));
        out
    }

    #[test]
    fn reads_mp4_length() {
        assert_eq!(parse_mp4(&mp4(0, 1000, 134_500)), Some(134.5));
        let p = tmp("cap.mp4");
        std::fs::write(&p, mp4(0, 1000, 134_500)).unwrap();
        assert_eq!(read_mp4(&p), Some(134.5));
        std::fs::write(&p, mp4(1, 90000, 900_000)).unwrap();
        assert_eq!(read_mp4(&p), Some(10.0));
        std::fs::write(&p, b"not a movie").unwrap();
        assert_eq!(read_mp4(&p), None);
    }
}
