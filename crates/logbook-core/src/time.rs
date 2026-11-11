//! Local time without a date crate: libc's `localtime_r` and `mktime`, and
//! the names the recorders give their files.

/// A broken-down local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
}

pub fn local(t: f64) -> Local {
    let secs = t.floor() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    Local {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        min: tm.tm_min as u32,
        sec: tm.tm_sec as u32,
    }
}

/// Unix seconds for a local time.
pub fn unix(l: Local) -> f64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = l.year - 1900;
    tm.tm_mon = l.month as i32 - 1;
    tm.tm_mday = l.day as i32;
    tm.tm_hour = l.hour as i32;
    tm.tm_min = l.min as i32;
    tm.tm_sec = l.sec as i32;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as f64 }
}

/// A meeting folder: `2026-11-11_10-02-00`.
pub fn meeting_name(t: f64) -> String {
    let l = local(t);
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        l.year, l.month, l.day, l.hour, l.min, l.sec
    )
}

/// A screen capture: `recording-20261111-100200.mp4`.
pub fn screen_name(t: f64) -> String {
    let l = local(t);
    format!(
        "recording-{:04}{:02}{:02}-{:02}{:02}{:02}.mp4",
        l.year, l.month, l.day, l.hour, l.min, l.sec
    )
}

/// A chunk in a meeting folder: `chunk-003-103200.wav`.
pub fn chunk_name(n: u32, t: f64) -> String {
    let l = local(t);
    format!("chunk-{n:03}-{:02}{:02}{:02}.wav", l.hour, l.min, l.sec)
}

fn num(s: &str) -> Option<u32> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// Reads a meeting folder name back.
pub fn parse_meeting_name(name: &str) -> Option<Local> {
    let (date, time) = name.split_once('_')?;
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split('-').collect();
    if d.len() != 3 || t.len() != 3 || d[0].len() != 4 {
        return None;
    }
    Some(Local {
        year: num(d[0])? as i32,
        month: num(d[1])?,
        day: num(d[2])?,
        hour: num(t[0])?,
        min: num(t[1])?,
        sec: num(t[2])?,
    })
}

/// Reads a screen capture name back.
pub fn parse_screen_name(name: &str) -> Option<Local> {
    let rest = name.strip_prefix("recording-")?.strip_suffix(".mp4")?;
    let (date, time) = rest.split_once('-')?;
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    Some(Local {
        year: num(&date[0..4])? as i32,
        month: num(&date[4..6])?,
        day: num(&date[6..8])?,
        hour: num(&time[0..2])?,
        min: num(&time[2..4])?,
        sec: num(&time[4..6])?,
    })
}

/// The chunk number in `chunk-003-103200.wav`.
pub fn chunk_number(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("chunk-")?.strip_suffix(".wav")?;
    num(rest.split('-').next()?)
}

/// `01:02:03`.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// A length for lists: `2:14` under ten minutes, `52 min`, `1 h 05`.
pub fn length(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s < 600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else if s < 3600 {
        format!("{} min", (s + 30) / 60)
    } else {
        let m = (s + 30) / 60;
        format!("{} h {:02}", m / 60, m % 60)
    }
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn weekday(t: f64) -> &'static str {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let secs = t.floor() as libc::time_t;
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    DAYS[tm.tm_wday.clamp(0, 6) as usize]
}

fn month(l: Local) -> &'static str {
    MONTHS[(l.month as usize).clamp(1, 12) - 1]
}

/// `today 10:02`, `yesterday 14:30`, `Mon 9 Nov 08:15`.
pub fn when(t: f64, now: f64) -> String {
    let l = local(t);
    let n = local(now);
    let hm = format!("{:02}:{:02}", l.hour, l.min);
    let noon = |x: Local| {
        unix(Local {
            hour: 12,
            min: 0,
            sec: 0,
            ..x
        })
    };
    match ((noon(n) - noon(l)) / 86400.0).round() as i64 {
        0 => format!("today {hm}"),
        1 => format!("yesterday {hm}"),
        _ => format!("{} {} {} {hm}", weekday(t), l.day, month(l)),
    }
}

/// `Wed 11 Nov 2026 10:02`, for a transcript's heading.
pub fn long(t: f64) -> String {
    let l = local(t);
    format!(
        "{} {} {} {} {:02}:{:02}",
        weekday(t),
        l.day,
        month(l),
        l.year,
        l.hour,
        l.min
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let t = unix(Local {
            year: 2026,
            month: 11,
            day: 11,
            hour: 10,
            min: 2,
            sec: 7,
        });
        assert_eq!(meeting_name(t), "2026-11-11_10-02-07");
        assert_eq!(screen_name(t), "recording-20261111-100207.mp4");
        assert_eq!(chunk_name(3, t), "chunk-003-100207.wav");
        assert_eq!(unix(parse_meeting_name("2026-11-11_10-02-07").unwrap()), t);
        assert_eq!(
            unix(parse_screen_name("recording-20261111-100207.mp4").unwrap()),
            t
        );
        assert_eq!(chunk_number("chunk-012-100207.wav"), Some(12));
        assert_eq!(parse_meeting_name("notes"), None);
        assert_eq!(parse_screen_name("recording-2026.mp4"), None);
    }

    #[test]
    fn clocks_and_lengths() {
        assert_eq!(clock(41.0 * 60.0 + 12.0), "00:41:12");
        assert_eq!(clock(3725.9), "01:02:05");
        assert_eq!(length(134.0), "2:14");
        assert_eq!(length(52.0 * 60.0 + 10.0), "52 min");
        assert_eq!(length(3900.0), "1 h 05");
    }

    #[test]
    fn days() {
        let now = unix(Local {
            year: 2026,
            month: 11,
            day: 11,
            hour: 9,
            min: 0,
            sec: 0,
        });
        assert_eq!(when(now - 3600.0, now), "today 08:00");
        assert_eq!(when(now - 86400.0, now), "yesterday 09:00");
        assert_eq!(when(now - 2.0 * 86400.0, now), "Mon 9 Nov 09:00");
        assert_eq!(long(now), "Wed 11 Nov 2026 09:00");
    }
}
