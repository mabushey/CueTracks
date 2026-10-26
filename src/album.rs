//! Album view: track list, durations, and the text that gets printed.

use crate::cue::{frames_from_samples, parse_cue, CueTime, ParsedCue, ParsedIndex, ParsedTrack};
use crate::error::{Error, Result};
use crate::flac::{CueSheet, FlacMeta};
use colored::Colorize;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioSpec {
    pub bits_per_sample: u32,
    pub sample_rate: u32,
    pub channels: u32,
}

/// Length of one audio file referenced by a cue sheet.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileAudio {
    pub end: Option<CueTime>,
    pub spec: Option<AudioSpec>,
}

impl FileAudio {
    pub fn from_flac(meta: &FlacMeta) -> Self {
        Self {
            end: end_time(meta),
            spec: Some(AudioSpec {
                bits_per_sample: meta.bits_per_sample,
                sample_rate: meta.sample_rate,
                channels: meta.channels,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Album {
    pub source: String,
    pub performer: Option<String>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub disc: Option<u16>,
    pub total_discs: Option<u16>,
    pub audio: Option<AudioSpec>,
    pub total: Option<CueTime>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub number: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub start: Option<CueTime>,
    pub duration: Option<CueTime>,
}

pub fn album_from_cue(source: &str, cue: &ParsedCue, files: &[FileAudio]) -> Result<Album> {
    if cue.files.iter().all(|file| file.tracks.is_empty()) {
        return Err(Error::new("no tracks in cue sheet"));
    }

    let ends = ends_of(cue, files);
    let mut total = Some(CueTime::from_frames(0));
    let mut audio = None;
    for (index, end) in ends.iter().enumerate() {
        if audio.is_none() {
            audio = files.get(index).and_then(|info| info.spec);
        }
        total = match (total, *end) {
            (Some(so_far), Some(end)) => Some(CueTime::from_frames(so_far.frames() + end.frames())),
            _ => None,
        };
    }
    let tracks = song_tracks(cue, &ends);

    Ok(Album {
        source: source.to_string(),
        performer: cue.performer.clone(),
        title: cue.title.clone(),
        date: cue.date.clone(),
        disc: cue.disc,
        total_discs: cue.total_discs,
        audio,
        total,
        tracks,
    })
}

pub fn album_from_flac(source: &str, meta: &FlacMeta) -> Result<Album> {
    if let Some(text) = meta.cue_text() {
        let cue = parse_cue(&text)?;
        let audio = if cue.files.len() == 1 {
            vec![FileAudio::from_flac(meta)]
        } else {
            // A multi-file sheet embedded in one FLAC does not say which file
            // this stream belongs to. Titles still come from the sheet.
            vec![FileAudio::default(); cue.files.len()]
        };
        let mut album = album_from_cue(source, &cue, &audio)?;
        if album.audio.is_none() {
            album.audio = FileAudio::from_flac(meta).spec;
        }
        return Ok(album);
    }

    let Some(sheet) = meta.cuesheet.as_ref() else {
        return Err(Error::new("no cue sheet found"));
    };
    if sheet.tracks.is_empty() {
        return Err(Error::new("no cue sheet found"));
    }
    let cue = cue_from_binary(meta, sheet);
    album_from_cue(source, &cue, &[FileAudio::from_flac(meta)])
}

fn cue_from_binary(meta: &FlacMeta, sheet: &CueSheet) -> ParsedCue {
    let rate = meta.sample_rate;
    let tracks = sheet
        .tracks
        .iter()
        .map(|track| ParsedTrack {
            number: u32::from(track.number),
            title: None,
            performer: None,
            indices: track
                .indices
                .iter()
                .filter_map(|(number, samples)| {
                    Some(ParsedIndex {
                        number: u32::from(*number),
                        file: 0,
                        time: frames_from_samples(*samples, rate)?,
                    })
                })
                .collect(),
        })
        .collect();
    let mut cue = ParsedCue {
        performer: first_tag(
            meta,
            &["ALBUMARTIST", "ALBUM ARTIST", "ARTIST", "PERFORMER"],
        ),
        title: first_tag(meta, &["ALBUM", "TITLE"]),
        date: first_tag(meta, &["DATE", "YEAR"]),
        disc: None,
        total_discs: None,
        files: vec![crate::cue::ParsedFile {
            name: String::new(),
            tracks,
        }],
    };
    apply_disc_tags(&mut cue, meta);
    cue
}

fn first_tag(meta: &FlacMeta, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| meta.tag(name))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn apply_disc_tags(cue: &mut ParsedCue, meta: &FlacMeta) {
    if let Some(value) = meta.tag("DISCNUMBER").or_else(|| meta.tag("DISC")) {
        let (disc, total) = split_disc(value);
        cue.disc = disc;
        cue.total_discs = cue.total_discs.or(total);
    }
    if cue.total_discs.is_none() {
        if let Some(value) = meta.tag("TOTALDISCS").or_else(|| meta.tag("DISCTOTAL")) {
            cue.total_discs = value.trim().parse().ok();
        }
    }
}

fn split_disc(value: &str) -> (Option<u16>, Option<u16>) {
    let value = value.trim();
    match value.split_once(['/', '-']) {
        Some((disc, total)) => (parse_u16(disc), parse_u16(total)),
        None => (parse_u16(value), None),
    }
}

fn parse_u16(value: &str) -> Option<u16> {
    value.trim().parse().ok()
}

fn ends_of(cue: &ParsedCue, files: &[FileAudio]) -> Vec<Option<CueTime>> {
    cue.files
        .iter()
        .enumerate()
        .map(|(index, _)| files.get(index).and_then(|info| info.end))
        .collect()
}

/// A song runs from its INDEX 01 to the next song's first later index in that same file.
///
/// Index times are relative to the current FILE, so they are never compared
/// across files. Sheets write INDEX 00 before INDEX 01, and that first index
/// is the pregap when one exists; the gap is not part of either song. A track
/// may open in one file (INDEX 00 at the end) and continue at INDEX 01 of the
/// next file. The length then starts at that INDEX 01 and stops at the next
/// index in the new file, or at the end of the file when this is its last index.
fn song_tracks(cue: &ParsedCue, ends: &[Option<CueTime>]) -> Vec<Track> {
    let flat: Vec<&ParsedTrack> = cue.files.iter().flat_map(|file| &file.tracks).collect();
    flat.iter()
        .enumerate()
        .map(|(index, track)| {
            let start_at = song_start(track);
            let duration = start_at.and_then(|(file, start)| {
                let end = end_in_file(
                    flat.get(index + 1).copied(),
                    file,
                    start,
                    ends.get(file).copied().flatten(),
                );
                end.and_then(|end| between(start, end))
            });
            Track {
                number: track.number,
                title: track.title.clone(),
                performer: track.performer.clone(),
                start: start_at.map(|(_, time)| time),
                duration,
            }
        })
        .collect()
}

/// INDEX 01 when that time still moves forward in its file, otherwise the earliest index that does.
///
/// A higher index with an earlier time than a lower index in the same file is
/// not a start. Sheets sometimes copy the previous song's INDEX 01 into that
/// slot. The song then starts at INDEX 00.
fn song_start(track: &ParsedTrack) -> Option<(usize, CueTime)> {
    let forward = |index: &&ParsedIndex| !goes_backwards(track, index);
    let index = track
        .indices
        .iter()
        .find(|index| index.number == 1 && forward(index))
        .or_else(|| {
            track
                .indices
                .iter()
                .filter(forward)
                .min_by_key(|index| index.time.frames())
        })?;
    Some((index.file, index.time))
}

/// True when a lower-numbered index in the same file is later than `index`.
fn goes_backwards(track: &ParsedTrack, index: &ParsedIndex) -> bool {
    track.indices.iter().any(|lower| {
        lower.number < index.number
            && lower.file == index.file
            && lower.time.frames() > index.time.frames()
    })
}

/// End of `start` inside `file`.
///
/// The next track's first index at or after `start` in this file wins. When
/// the next track has no index here, the song runs to the end of the file.
/// An index that lands before `start` is not an end point; using the file
/// end in that case would swallow the rest of the file.
fn end_in_file(
    next: Option<&ParsedTrack>,
    file: usize,
    start: CueTime,
    file_end: Option<CueTime>,
) -> Option<CueTime> {
    let Some(next) = next else {
        return file_end;
    };
    let mut saw_file = false;
    for index in &next.indices {
        if index.file != file {
            continue;
        }
        saw_file = true;
        if goes_backwards(next, index) {
            continue;
        }
        if index.time.frames() >= start.frames() {
            return Some(index.time);
        }
    }
    if saw_file {
        None
    } else {
        file_end
    }
}

fn between(start: CueTime, end: CueTime) -> Option<CueTime> {
    if end.frames() < start.frames() {
        None
    } else {
        Some(CueTime::from_frames(end.frames() - start.frames()))
    }
}

fn end_time(meta: &FlacMeta) -> Option<CueTime> {
    let lead_out = meta
        .cuesheet
        .as_ref()
        .and_then(|sheet| sheet.lead_out_samples)
        .filter(|samples| *samples > 0);
    let samples = match (lead_out, meta.samples) {
        (Some(lead), Some(total)) => Some(lead.min(total)),
        (Some(lead), None) => Some(lead),
        (None, total) => total,
    }?;
    frames_from_samples(samples, meta.sample_rate)
}

pub fn render(album: &Album, color: bool, show_source: bool) -> String {
    let mut out = String::new();
    if show_source {
        let _ = writeln!(out, "{}", paint(&album.source, color, |text| text.dimmed()));
    }
    let _ = writeln!(out, "{}", paint(&heading(album), color, |text| text.bold()));
    let subtitle = subtitle(album);
    if !subtitle.is_empty() {
        let _ = writeln!(out, "{}", paint(&subtitle, color, |text| text.dimmed()));
    }
    out.push('\n');

    if album.tracks.is_empty() {
        out.push_str("  (no tracks)\n");
        return out;
    }

    let labels: Vec<String> = album
        .tracks
        .iter()
        .map(|track| track_label(album, track))
        .collect();
    let starts: Vec<String> = album
        .tracks
        .iter()
        .map(|track| {
            track
                .start
                .map(CueTime::format_index)
                .unwrap_or_else(|| "?".to_string())
        })
        .collect();
    let durations: Vec<String> = album
        .tracks
        .iter()
        .map(|track| {
            track
                .duration
                .map(CueTime::format_duration)
                .unwrap_or_else(|| "?".to_string())
        })
        .collect();

    let num_w = if album.tracks.iter().any(|track| track.number >= 100) {
        3
    } else {
        2
    };
    let title_w = labels
        .iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0)
        .max("Title".len());
    let start_w = starts
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("Start".len());
    let dur_w = durations
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("Length".len());

    let header = format!(
        " {:>num_w$}  {:<title_w$}  {:>start_w$}  {:>dur_w$}",
        "#", "Title", "Start", "Length"
    );
    let _ = writeln!(out, "{}", paint(&header, color, |text| text.dimmed()));
    for (index, track) in album.tracks.iter().enumerate() {
        let number = format!("{:0width$}", track.number, width = num_w);
        let title = format!("{:<title_w$}", labels[index]);
        let start = format!("{:>start_w$}", starts[index]);
        let duration = format!("{:>dur_w$}", durations[index]);
        let _ = writeln!(
            out,
            " {}  {}  {}  {}",
            paint(&number, color, |text| text.cyan()),
            title,
            paint(&start, color, |text| text.dimmed()),
            paint(&duration, color, |text| text.green()),
        );
    }
    out
}

fn heading(album: &Album) -> String {
    match (&album.performer, &album.title) {
        (Some(performer), Some(title)) => format!("{performer} — {title}"),
        (Some(performer), None) => performer.clone(),
        (None, Some(title)) => title.clone(),
        (None, None) => album.source.clone(),
    }
}

fn subtitle(album: &Album) -> String {
    let mut parts = Vec::new();
    if let Some(date) = album.date.as_deref() {
        parts.push(display_date(date).to_string());
    }
    if let Some(disc) = album.disc {
        parts.push(match album.total_discs {
            Some(total) => format!("CD {disc}/{total}"),
            None => format!("CD {disc}"),
        });
    }
    if let Some(audio) = album.audio {
        parts.push(format!(
            "{}-bit/{} {}",
            audio.bits_per_sample,
            format_rate(audio.sample_rate),
            format_channels(audio.channels)
        ));
    }
    if let Some(total) = album.total {
        parts.push(total.format_duration());
    }
    parts.join(" · ")
}

fn display_date(date: &str) -> &str {
    let bytes = date.as_bytes();
    if bytes.len() >= 5
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && (bytes[4] == b'-' || bytes[4] == b'/')
    {
        &date[..4]
    } else {
        date
    }
}

fn format_rate(hz: u32) -> String {
    match hz {
        44_100 => "44.1 kHz".to_string(),
        48_000 => "48 kHz".to_string(),
        88_200 => "88.2 kHz".to_string(),
        96_000 => "96 kHz".to_string(),
        176_400 => "176.4 kHz".to_string(),
        192_000 => "192 kHz".to_string(),
        n if n % 1000 == 0 => format!("{} kHz", n / 1000),
        n => format!("{n} Hz"),
    }
}

fn format_channels(channels: u32) -> String {
    match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n}ch"),
    }
}

fn track_label(album: &Album, track: &Track) -> String {
    let title = track
        .title
        .clone()
        .unwrap_or_else(|| format!("Track {:02}", track.number));
    match track.performer.as_deref() {
        Some(performer) if !same_name(Some(performer), album.performer.as_deref()) => {
            format!("{performer} — {title}")
        }
        _ => title,
    }
}

fn same_name(track: Option<&str>, album: Option<&str>) -> bool {
    match (track, album) {
        (Some(track), Some(album)) => track.trim().eq_ignore_ascii_case(album.trim()),
        _ => false,
    }
}

fn paint(text: &str, color: bool, style: impl Fn(&str) -> colored::ColoredString) -> String {
    if color {
        style(text).to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::parse_cue;
    use crate::flac::{streaminfo_payload, wrap_blocks, CueTrack, FlacMeta};
    use std::io::Cursor;

    fn audio(end: CueTime) -> FileAudio {
        FileAudio {
            end: Some(end),
            spec: Some(AudioSpec {
                bits_per_sample: 16,
                sample_rate: 44_100,
                channels: 2,
            }),
        }
    }

    #[test]
    fn pregap_is_left_out_of_both_songs_and_the_last_uses_the_file() {
        let cue = parse_cue(
            r#"
            PERFORMER "Impellitteri"
            TITLE "System X"
            REM DATE 2002
            FILE "a.flac" WAVE
              TRACK 01 AUDIO
                TITLE "United We Stand"
                INDEX 01 00:00:00
              TRACK 02 AUDIO
                TITLE "Perfect Crime"
                PERFORMER "Someone Else"
                INDEX 00 03:03:31
                INDEX 01 03:04:10
            "#,
        )
        .unwrap();
        let album =
            album_from_cue("System X.cue", &cue, &[audio(CueTime::from_parts(4, 0, 0))]).unwrap();
        assert_eq!(
            album.tracks[0].duration,
            Some(CueTime::from_parts(3, 3, 31))
        );
        assert_eq!(album.tracks[1].start, Some(CueTime::from_parts(3, 4, 10)));
        assert_eq!(
            album.tracks[1].duration,
            Some(CueTime::from_frames(
                CueTime::from_parts(4, 0, 0).frames() - CueTime::from_parts(3, 4, 10).frames()
            ))
        );
        let text = render(&album, false, false);
        assert!(text.contains("Impellitteri — System X"));
        assert!(text.contains("2002 · 16-bit/44.1 kHz stereo · 4:00"));
        assert!(text.contains("Someone Else — Perfect Crime"));
        assert!(text.contains("3:04"));
        assert!(!text.contains("System X.cue"));
    }

    #[test]
    fn durations_do_not_cross_files_and_an_hour_is_labeled() {
        let cue = parse_cue(
            r#"
            FILE "a.flac" WAVE
              TRACK 01 AUDIO
                TITLE "One"
                INDEX 01 00:00:00
              TRACK 02 AUDIO
                TITLE "Two"
                INDEX 01 01:00:00
            FILE "b.flac" WAVE
              TRACK 03 AUDIO
                TITLE "Three"
                INDEX 01 00:00:00
            "#,
        )
        .unwrap();
        let album = album_from_cue(
            "split.cue",
            &cue,
            &[
                audio(CueTime::from_parts(1, 30, 0)),
                audio(CueTime::from_parts(0, 30, 0)),
            ],
        )
        .unwrap();
        assert_eq!(album.tracks[0].duration, Some(CueTime::from_parts(1, 0, 0)));
        assert_eq!(
            album.tracks[1].duration,
            Some(CueTime::from_parts(0, 30, 0))
        );
        assert_eq!(
            album.tracks[2].duration,
            Some(CueTime::from_parts(0, 30, 0))
        );
        assert_eq!(album.total, Some(CueTime::from_parts(2, 0, 0)));
        assert_eq!(album.tracks[0].duration.unwrap().format_duration(), "1:00");
        assert_eq!(CueTime::from_parts(60, 0, 0).format_duration(), "1:00:00");
    }

    #[test]
    fn a_pregap_at_the_end_of_a_file_does_not_lengthen_the_next_song() {
        // The next track is opened in this file at INDEX 00, then INDEX 01
        // continues it at the start of the following file.
        let cue = parse_cue(
            r#"
            REM REPLAYGAIN_ALBUM_GAIN -6.17 dB
            FILE "01.flac" WAVE
              TRACK 01 AUDIO
                TITLE "Everything"
                PREGAP 00:00:32
                INDEX 01 00:00:00
            FILE "02.flac" WAVE
              TRACK 02 AUDIO
                TITLE "Weight"
                INDEX 01 00:00:00
              TRACK 03 AUDIO
                TITLE "Beautiful"
                INDEX 00 04:03:40
            FILE "03.flac" WAVE
                INDEX 01 00:00:00
              TRACK 04 AUDIO
                TITLE "Sad"
                INDEX 00 03:32:30
            FILE "04.flac" WAVE
                INDEX 01 00:00:00
            "#,
        )
        .unwrap();
        let lengths = [
            CueTime::from_parts(3, 23, 13),
            CueTime::from_parts(4, 4, 15),
            CueTime::from_parts(3, 40, 0),
            CueTime::from_parts(4, 8, 7),
        ];
        let album = album_from_cue("passion.cue", &cue, &lengths.map(audio)).unwrap();
        assert_eq!(album.tracks[0].start, Some(CueTime::from_parts(0, 0, 0)));
        assert_eq!(album.tracks[0].duration, Some(lengths[0]));
        assert_eq!(
            album.tracks[1].duration,
            Some(CueTime::from_parts(4, 3, 40))
        );
        assert_eq!(album.tracks[2].start, Some(CueTime::from_parts(0, 0, 0)));
        // File 03 is longer than INDEX 00, so this checks the pregap is excluded.
        assert_eq!(
            album.tracks[2].duration,
            Some(CueTime::from_parts(3, 32, 30))
        );
        assert_eq!(album.tracks[3].duration, Some(lengths[3]));
        let frames: i64 = lengths.iter().map(|time| time.frames()).sum();
        assert_eq!(album.total, Some(CueTime::from_frames(frames)));
    }

    #[test]
    fn a_later_index_with_an_earlier_time_does_not_zero_the_previous_song() {
        let cue = parse_cue(
            r#"
            FILE "album.flac" WAVE
              TRACK 02 AUDIO
                TITLE "Weight"
                INDEX 01 03:23:45
              TRACK 03 AUDIO
                TITLE "Beautiful"
                INDEX 00 07:27:10
                INDEX 01 03:23:45
              TRACK 04 AUDIO
                TITLE "Sad"
                INDEX 01 11:00:15
            "#,
        )
        .unwrap();
        let album =
            album_from_cue("merged.cue", &cue, &[audio(CueTime::from_parts(44, 5, 0))]).unwrap();
        assert_eq!(
            album.tracks[0].duration,
            Some(CueTime::from_parts(4, 3, 40))
        );
        // INDEX 01 repeats the previous song and is earlier than INDEX 00,
        // so the song starts at INDEX 00 instead of overlapping track 2.
        assert_eq!(album.tracks[1].start, Some(CueTime::from_parts(7, 27, 10)));
        assert_eq!(
            album.tracks[1].duration,
            Some(CueTime::from_frames(
                CueTime::from_parts(11, 0, 15).frames() - CueTime::from_parts(7, 27, 10).frames()
            ))
        );
        assert_eq!(album.tracks[1].start.unwrap().format_index(), "7:27");
        assert_eq!(album.tracks[1].duration.unwrap().format_duration(), "3:33");
    }

    #[test]
    fn missing_audio_leaves_the_last_length_unknown() {
        let cue =
            parse_cue("FILE \"a.flac\" WAVE\nTRACK 01 AUDIO\nTITLE \"Only\"\nINDEX 01 00:00:00\n")
                .unwrap();
        let album = album_from_cue("a.cue", &cue, &[]).unwrap();
        assert_eq!(album.tracks[0].duration, None);
        assert!(render(&album, false, true).contains('?'));
    }

    #[test]
    fn marillion_sheet_matches_the_index_boundaries() {
        let text = include_str!("../tests/Marillion - Misplaced Childhood (CD2).flac.cue");
        let cue = parse_cue(text).unwrap();
        let album = album_from_cue("marillion.cue", &cue, &[FileAudio::default()]).unwrap();
        assert_eq!(album.disc, Some(2));
        assert_eq!(album.total_discs, Some(2));
        assert_eq!(album.date.as_deref(), Some("1985"));
        assert_eq!(album.tracks.len(), 17);
        assert_eq!(album.tracks[0].title.as_deref(), Some("Lady Nina"));
        // INDEX 00 of track 2 is 05:47:50, so track 1 does not include the pregap.
        assert_eq!(
            album.tracks[0].duration,
            Some(CueTime::from_parts(5, 47, 50))
        );
        assert_eq!(
            album.tracks[1].duration,
            Some(CueTime::from_parts(4, 5, 50))
        );
        assert_eq!(album.tracks[1].start, Some(CueTime::from_parts(5, 50, 10)));
        assert_eq!(
            album.tracks[16].title.as_deref(),
            Some("White Feather (Album Demo)")
        );
        assert_eq!(album.tracks[16].duration, None);
        let shown = render(&album, false, false);
        assert!(shown.contains("CD 2/2"));
        assert!(shown.contains("5:48"));
        assert!(shown.contains("Marillion — Misplaced Childhood (CD2: Demo)"));
    }

    #[test]
    fn binary_cuesheet_uses_tags_and_lead_out() {
        let mut meta = FlacMeta {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            samples: Some(44_100 * 60),
            tags: vec![
                ("ARTIST".into(), "Test".into()),
                ("ALBUM".into(), "Demo".into()),
                ("DATE".into(), "1999-05-01".into()),
                ("DISCNUMBER".into(), "1/2".into()),
            ],
            cuesheet: Some(CueSheet {
                lead_out_samples: Some(44_100 * 60),
                tracks: vec![
                    CueTrack {
                        number: 1,
                        indices: vec![(1, 0)],
                    },
                    CueTrack {
                        number: 2,
                        indices: vec![(0, 44_100 * 30), (1, 44_100 * 31)],
                    },
                ],
            }),
        };
        let album = album_from_flac("demo.flac", &meta).unwrap();
        assert_eq!(album.performer.as_deref(), Some("Test"));
        assert_eq!(album.title.as_deref(), Some("Demo"));
        assert_eq!(album.disc, Some(1));
        assert_eq!(album.total_discs, Some(2));
        assert_eq!(album.tracks[0].title, None);
        assert_eq!(album.tracks[0].duration.unwrap().format_duration(), "0:30");
        assert_eq!(album.tracks[1].start.unwrap().format_index(), "0:31");
        assert_eq!(album.tracks[1].duration.unwrap().format_duration(), "0:29");
        assert!(render(&album, false, false).contains("1999 · CD 1/2"));
        meta.cuesheet = None;
        assert!(album_from_flac("demo.flac", &meta).is_err());
    }

    #[test]
    fn embedded_text_cue_sets_the_last_track_from_streaminfo() {
        let info = streaminfo_payload(44_100, 2, 16, 44_100 * 10);
        let comment = vorbis(&[
            ("CUESHEET", "PERFORMER \"Ada\"\nTITLE \"Hour\"\nFILE \"x.flac\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Only\"\n    INDEX 01 00:00:00\n"),
        ]);
        let bytes = wrap_blocks(&[(0, &info), (4, &comment)]);
        let meta = crate::flac::read_flac_reader(&mut Cursor::new(bytes)).unwrap();
        let album = album_from_flac("hour.flac", &meta).unwrap();
        assert_eq!(album.tracks[0].title.as_deref(), Some("Only"));
        assert_eq!(album.tracks[0].duration.unwrap().format_duration(), "0:10");
        assert_eq!(album.total.unwrap().format_duration(), "0:10");
    }

    fn vorbis(tags: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(tags.len() as u32).to_le_bytes());
        for (name, value) in tags {
            let comment = format!("{name}={value}");
            out.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            out.extend_from_slice(comment.as_bytes());
        }
        out
    }
}
