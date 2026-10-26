//! Text cue sheet parser.
//!
//! Times use the cue-sheet clock: 75 frames per second, written `mm:ss:ff`.
//! Minutes are not limited to 59.

use crate::error::{Error, Result};
use std::fs;
use std::path::Path;

/// One cue-sheet timestamp, stored as a frame count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CueTime {
    frames: i64,
}

impl CueTime {
    pub fn from_frames(frames: i64) -> Self {
        Self { frames }
    }

    pub fn from_parts(mins: i64, secs: i64, frames: i64) -> Self {
        Self {
            frames: (mins * 60 + secs) * 75 + frames,
        }
    }

    pub fn frames(self) -> i64 {
        self.frames
    }

    /// `m:ss` or `h:mm:ss`, with leftover frames dropped.
    ///
    /// Use this for index positions so the printed time matches the cue sheet.
    pub fn format_index(self) -> String {
        format_hms(self.frames.unsigned_abs() / 75, self.frames < 0)
    }

    /// `m:ss` or `h:mm:ss`, rounded to the nearest second.
    pub fn format_duration(self) -> String {
        let frames = self.frames.unsigned_abs();
        // 37.5 frames is half a second. Frame counts are integers, so 38
        // frames and above round up and 37 frames and below round down.
        let secs = frames.saturating_add(37) / 75;
        format_hms(secs, self.frames < 0)
    }
}

fn format_hms(total_secs: u64, negative: bool) -> String {
    let secs = total_secs % 60;
    let mins_total = total_secs / 60;
    let body = if mins_total >= 60 {
        format!("{}:{:02}:{:02}", mins_total / 60, mins_total % 60, secs)
    } else {
        format!("{mins_total}:{secs:02}")
    };
    if negative {
        format!("-{body}")
    } else {
        body
    }
}

/// Convert an inter-channel sample count into cue frames, rounding to the nearest frame.
pub fn frames_from_samples(samples: u64, sample_rate: u32) -> Option<CueTime> {
    if sample_rate == 0 {
        return None;
    }
    let samples = u128::from(samples);
    let rate = u128::from(sample_rate);
    let frames = (samples * 75 + rate / 2) / rate;
    i64::try_from(frames).ok().map(CueTime::from_frames)
}

/// A cue sheet reduced to the fields this program displays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedCue {
    pub performer: Option<String>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub disc: Option<u16>,
    pub total_discs: Option<u16>,
    pub files: Vec<ParsedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedFile {
    pub name: String,
    pub tracks: Vec<ParsedTrack>,
}

/// One `INDEX` line. The time is relative to `file`, not to the album.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedIndex {
    pub number: u32,
    pub file: usize,
    pub time: CueTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTrack {
    pub number: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    /// Index 1 is the start of the song. A later `FILE` can add indexes to
    /// the track that is already open; those indexes belong to the new file.
    pub indices: Vec<ParsedIndex>,
}

pub fn read_sheet(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(decode_cue_bytes(&bytes))
}

pub fn decode_cue_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        // EAC and other Windows rippers write cue sheets in windows-1252.
        Err(_) => bytes.iter().copied().map(decode_cp1252).collect(),
    }
}

fn decode_cp1252(byte: u8) -> char {
    match byte {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        _ => byte as char,
    }
}

pub fn parse_cue(text: &str) -> Result<ParsedCue> {
    let owned;
    let text = if text.contains('\r') {
        owned = text.replace("\r\n", "\n").replace('\r', "\n");
        owned.as_str()
    } else {
        text
    };

    let mut sheet = ParsedCue::default();
    let mut file_at: Option<usize> = None;
    // `(file that holds the TRACK command, index in that file's track list)`.
    // A new FILE does not clear this: the next INDEX still belongs to the open track.
    let mut open: Option<(usize, usize)> = None;

    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Err(err) = apply_line(&mut sheet, &mut file_at, &mut open, line) {
            return Err(Error::new(format!("line {line_no}: {err}")));
        }
    }
    Ok(sheet)
}

fn apply_line(
    sheet: &mut ParsedCue,
    file_at: &mut Option<usize>,
    open: &mut Option<(usize, usize)>,
    line: &str,
) -> Result<()> {
    let Some((cmd, rest)) = split_command(line) else {
        return Ok(());
    };
    apply_command(sheet, file_at, open, &cmd, rest)
}

fn apply_command(
    sheet: &mut ParsedCue,
    file_at: &mut Option<usize>,
    open: &mut Option<(usize, usize)>,
    cmd: &str,
    rest: &str,
) -> Result<()> {
    match cmd {
        "REM" => apply_rem(sheet, rest),
        "PERFORMER" => {
            let (value, _) = take_token(rest);
            let value = blank_to_none(value);
            if let Some(track) = current_track(sheet, *open) {
                track.performer = value;
            } else {
                sheet.performer = value;
            }
        }
        "TITLE" => {
            let (value, _) = take_token(rest);
            let value = blank_to_none(value);
            if let Some(track) = current_track(sheet, *open) {
                track.title = value;
            } else {
                sheet.title = value;
            }
        }
        "FILE" => {
            let (name, _) = take_token(rest);
            if name.is_empty() {
                return Err(Error::new("FILE command is missing a name"));
            }
            // Cue sheets written on Windows use backslashes. Keep the name
            // usable as a relative path on every platform.
            let name = name.replace('\\', "/");
            sheet.files.push(ParsedFile {
                name,
                tracks: Vec::new(),
            });
            *file_at = Some(sheet.files.len() - 1);
        }
        "TRACK" => {
            let (number, _) = take_token(rest);
            let number: u32 = number
                .parse()
                .map_err(|_| Error::new(format!("invalid track number '{number}'")))?;
            if file_at.is_none() {
                sheet.files.push(ParsedFile {
                    name: String::new(),
                    tracks: Vec::new(),
                });
                *file_at = Some(sheet.files.len() - 1);
            }
            let Some(index) = *file_at else {
                return Err(Error::new("TRACK appears before FILE"));
            };
            let file = &mut sheet.files[index];
            file.tracks.push(ParsedTrack {
                number,
                title: None,
                performer: None,
                indices: Vec::new(),
            });
            *open = Some((index, file.tracks.len() - 1));
        }
        "INDEX" => {
            let file = (*file_at).ok_or_else(|| Error::new("INDEX appears before FILE"))?;
            let track = current_track(sheet, *open)
                .ok_or_else(|| Error::new("INDEX appears before TRACK"))?;
            let (number, rest) = take_token(rest);
            let (time, _) = take_token(rest);
            let number: u32 = number
                .parse()
                .map_err(|_| Error::new(format!("invalid index number '{number}'")))?;
            let time = parse_time(&time)?;
            track.indices.push(ParsedIndex { number, file, time });
        }
        // PREGAP is silence a burner inserts. It is not audio in the file,
        // so it does not change the track lengths we report.
        _ => {}
    }
    Ok(())
}

fn apply_rem(sheet: &mut ParsedCue, rest: &str) {
    let Some((key, value)) = split_command(rest) else {
        return;
    };
    let (value, _) = take_token(value);
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    match key.as_str() {
        "DATE" | "YEAR" => sheet.date = Some(value.to_string()),
        "DISCNUMBER" | "DISC" => {
            let (disc, total) = parse_disc(value);
            if disc.is_some() {
                sheet.disc = disc;
            }
            if total.is_some() {
                sheet.total_discs = total;
            }
        }
        "TOTALDISCS" | "DISCTOTAL" => {
            if let Some(n) = parse_u16(value) {
                sheet.total_discs = Some(n);
            }
        }
        _ => {}
    }
}

fn parse_disc(value: &str) -> (Option<u16>, Option<u16>) {
    match value.split_once(['/', '-']) {
        Some((disc, total)) => (parse_u16(disc), parse_u16(total)),
        None => (parse_u16(value), None),
    }
}

fn parse_u16(value: &str) -> Option<u16> {
    value.trim().parse().ok()
}

fn current_track(sheet: &mut ParsedCue, open: Option<(usize, usize)>) -> Option<&mut ParsedTrack> {
    let (file, track) = open?;
    sheet.files.get_mut(file)?.tracks.get_mut(track)
}

fn blank_to_none(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn split_command(line: &str) -> Option<(String, &str)> {
    let line = line.trim();
    let cmd = line.split_whitespace().next()?;
    let rest = line[cmd.len()..].trim();
    Some((cmd.to_ascii_uppercase(), rest))
}

pub fn parse_time(token: &str) -> Result<CueTime> {
    let mut parts = token.split(':');
    let (Some(mins), Some(secs), Some(frames), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::new(format!("invalid time '{token}'")));
    };
    let mins: i64 = mins
        .parse()
        .map_err(|_| Error::new(format!("invalid time '{token}'")))?;
    let secs: i64 = secs
        .parse()
        .map_err(|_| Error::new(format!("invalid time '{token}'")))?;
    let frames: i64 = frames
        .parse()
        .map_err(|_| Error::new(format!("invalid time '{token}'")))?;
    if mins < 0 || !(0..60).contains(&secs) || !(0..75).contains(&frames) {
        return Err(Error::new(format!("invalid time '{token}'")));
    }
    Ok(CueTime::from_parts(mins, secs, frames))
}

/// Read one cue token. Quoted tokens honor `\"` escapes.
fn take_token(input: &str) -> (String, &str) {
    let s = input.trim_start();
    if s.is_empty() {
        return (String::new(), s);
    }
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut end = 1usize;
        let mut chars = rest.char_indices();
        while let Some((i, ch)) = chars.next() {
            if ch == '\\' {
                // Only `\"` is an escape. A Windows path's backslashes are literal.
                if let Some((j, escaped)) = chars.next() {
                    if escaped != '"' {
                        out.push('\\');
                    }
                    out.push(escaped);
                    end = 1 + j + escaped.len_utf8();
                } else {
                    out.push('\\');
                    end = s.len();
                    break;
                }
            } else if ch == '"' {
                end = 1 + i + ch.len_utf8();
                break;
            } else {
                out.push(ch);
                end = 1 + i + ch.len_utf8();
            }
        }
        (out, s[end..].trim_start())
    } else {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        (s[..end].to_string(), s[end..].trim_start())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formatting_rounds_duration_and_keeps_index_text() {
        let index = CueTime::from_parts(5, 50, 10);
        assert_eq!(index.format_index(), "5:50");
        let almost = CueTime::from_parts(5, 47, 37);
        let over = CueTime::from_parts(5, 47, 38);
        assert_eq!(almost.format_duration(), "5:47");
        assert_eq!(over.format_duration(), "5:48");
        assert_eq!(CueTime::from_parts(60, 0, 0).format_index(), "1:00:00");
        assert_eq!(CueTime::from_parts(59, 59, 74).format_duration(), "1:00:00");
    }

    #[test]
    fn sample_count_from_the_notes_is_one_hour_thirteen() {
        // 195448260 samples at 44100 Hz is 4431.933 seconds, which rounds to 1:13:52.
        let time = frames_from_samples(195_448_260, 44_100).unwrap();
        assert_eq!(time.format_duration(), "1:13:52");
    }

    #[test]
    fn parses_pregap_as_the_end_of_the_previous_song() {
        let sheet = parse_cue(
            r#"
            PERFORMER "Impellitteri"
            TITLE "System X"
            REM DATE 2002
            FILE "Impellitteri - System X.flac" WAVE
              TRACK 01 AUDIO
                TITLE "United We Stand"
                INDEX 01 00:00:00
              TRACK 02 AUDIO
                TITLE "Perfect Crime"
                INDEX 00 03:03:31
                INDEX 01 03:04:10
            "#,
        )
        .unwrap();
        assert_eq!(sheet.performer.as_deref(), Some("Impellitteri"));
        assert_eq!(sheet.date.as_deref(), Some("2002"));
        assert_eq!(sheet.files.len(), 1);
        let tracks = &sheet.files[0].tracks;
        assert_eq!(tracks[0].title.as_deref(), Some("United We Stand"));
        assert_eq!(
            tracks[1].indices[0],
            ParsedIndex {
                number: 0,
                file: 0,
                time: CueTime::from_parts(3, 3, 31),
            }
        );
        assert_eq!(
            tracks[1].indices[1],
            ParsedIndex {
                number: 1,
                file: 0,
                time: CueTime::from_parts(3, 4, 10),
            }
        );
    }

    #[test]
    fn an_index_after_a_new_file_stays_on_the_open_track() {
        // CUETools appends the next track's pregap to the previous file, then
        // continues that same track at 00:00:00 of the next file.
        let sheet = parse_cue(
            r#"
            REM REPLAYGAIN_ALBUM_GAIN -6.17 dB
            FILE "02.flac" WAVE
              TRACK 02 AUDIO
                TITLE "Weight"
                INDEX 01 00:00:00
              TRACK 03 AUDIO
                TITLE "Beautiful"
                INDEX 00 04:03:40
            FILE "03.flac" WAVE
                INDEX 01 00:00:00
            "#,
        )
        .unwrap();
        assert_eq!(sheet.files[1].tracks.len(), 0);
        let continued = &sheet.files[0].tracks[1];
        assert_eq!(continued.title.as_deref(), Some("Beautiful"));
        assert_eq!(continued.indices[0].file, 0);
        assert_eq!(continued.indices[0].number, 0);
        assert_eq!(continued.indices[1].file, 1);
        assert_eq!(continued.indices[1].number, 1);
        assert_eq!(continued.indices[1].time, CueTime::from_parts(0, 0, 0));
    }

    #[test]
    fn lower_case_commands_quotes_and_crlf() {
        let sheet = parse_cue("performer \"A \\\"Band\\\"\"\r\ntitle Song\r\nfile song.flac WAVE\r\ntrack 1 audio\r\nindex 1 0:00:00\r\n").unwrap();
        assert_eq!(sheet.performer.as_deref(), Some("A \"Band\""));
        assert_eq!(sheet.title.as_deref(), Some("Song"));
        assert_eq!(sheet.files[0].name, "song.flac");
        assert_eq!(sheet.files[0].tracks[0].number, 1);
    }

    #[test]
    fn windows_1252_and_utf16_sheets_decode() {
        assert_eq!(decode_cue_bytes(&[0x93]), "\u{201C}");
        assert_eq!(decode_cue_bytes(&[0xEF, 0xBB, 0xBF, b'A']), "A");
        assert_eq!(decode_cue_bytes(&[0xFF, 0xFE, b'A', 0]), "A");
        let utf8 = "é".as_bytes();
        assert_eq!(decode_cue_bytes(utf8), "é");
    }

    #[test]
    fn disc_number_may_carry_the_total() {
        let sheet = parse_cue("REM DISCNUMBER 2/3\nREM DATE 1985-03-14\n").unwrap();
        assert_eq!(sheet.disc, Some(2));
        assert_eq!(sheet.total_discs, Some(3));
        assert_eq!(sheet.date.as_deref(), Some("1985-03-14"));
    }

    #[test]
    fn index_before_track_names_the_line() {
        let err = parse_cue("INDEX 01 00:00:00\n").unwrap_err();
        assert!(err.to_string().contains("line 1"));
    }

    #[test]
    fn windows_path_separators_are_normalized() {
        let sheet =
            parse_cue("FILE \"C:\\Music\\Album.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n")
                .unwrap();
        assert_eq!(sheet.files[0].name, "C:/Music/Album.flac");
    }
}
