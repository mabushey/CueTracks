//! FLAC metadata reader.
//!
//! Only the metadata blocks are read. Audio frames are left untouched.
//! A cuesheet may live in the CUESHEET block, or as a `CUESHEET` / `CUE`
//! Vorbis comment holding the text of a cue sheet.

use crate::error::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlacMeta {
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub samples: Option<u64>,
    pub tags: Vec<(String, String)>,
    pub cuesheet: Option<CueSheet>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueSheet {
    pub tracks: Vec<CueTrack>,
    /// Sample offset of the lead-out track, when the block has one.
    pub lead_out_samples: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueTrack {
    pub number: u8,
    /// `(index number, absolute sample offset from the start of the audio)`.
    pub indices: Vec<(u8, u64)>,
}

impl FlacMeta {
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Text cue sheet stored in a Vorbis comment, if the file has one.
    pub fn cue_text(&self) -> Option<String> {
        for key in ["CUESHEET", "CUE"] {
            let Some(value) = self.tag(key) else {
                continue;
            };
            let text = unescape_newlines(value);
            if looks_like_cue(&text) {
                return Some(text);
            }
        }
        None
    }

    /// True when the file carries a text cue sheet or a binary CUESHEET block.
    pub fn has_embedded_cue(&self) -> bool {
        self.cue_text().is_some()
            || self
                .cuesheet
                .as_ref()
                .is_some_and(|sheet| !sheet.tracks.is_empty())
    }
}

pub fn read_flac(path: &Path) -> Result<FlacMeta> {
    let mut file = File::open(path)?;
    read_flac_reader(&mut file)
}

pub fn read_flac_reader<R: Read + Seek>(input: &mut R) -> Result<FlacMeta> {
    expect_flac(input)?;
    let mut meta = FlacMeta::default();
    let mut saw_streaminfo = false;
    loop {
        let header = read_array::<_, 4>(input)?;
        let is_last = header[0] & 0x80 != 0;
        let kind = header[0] & 0x7f;
        let length =
            (u32::from(header[1]) << 16) | (u32::from(header[2]) << 8) | u32::from(header[3]);
        match kind {
            0 => {
                if length != 34 {
                    return Err(Error::new("invalid streaminfo block"));
                }
                let data = read_vec(input, 34)?;
                let info = parse_streaminfo(&data)?;
                if !saw_streaminfo {
                    meta.sample_rate = info.sample_rate;
                    meta.channels = info.channels;
                    meta.bits_per_sample = info.bits_per_sample;
                    meta.samples = info.samples;
                    saw_streaminfo = true;
                }
            }
            4 => {
                if length > 10 * 1024 * 1024 {
                    return Err(Error::new("Vorbis comment block is too large"));
                }
                let data = read_vec(input, length as usize)?;
                meta.tags.extend(parse_vorbis(&data));
            }
            5 => {
                if length > 1024 * 1024 {
                    return Err(Error::new("cuesheet block is too large"));
                }
                let data = read_vec(input, length as usize)?;
                meta.cuesheet = Some(parse_cuesheet(&data)?);
            }
            127 => return Err(Error::new("invalid FLAC metadata block")),
            _ => discard(input, u64::from(length))?,
        }
        if is_last {
            break;
        }
    }
    if !saw_streaminfo {
        return Err(Error::new("FLAC file is missing streaminfo"));
    }
    Ok(meta)
}

struct StreamBits {
    sample_rate: u32,
    channels: u32,
    bits_per_sample: u32,
    samples: Option<u64>,
}

fn parse_streaminfo(b: &[u8]) -> Result<StreamBits> {
    if b.len() != 34 {
        return Err(Error::new("invalid streaminfo block"));
    }
    let min_block = u16::from_be_bytes([b[0], b[1]]);
    let max_block = u16::from_be_bytes([b[2], b[3]]);
    if min_block < 16 || min_block > max_block {
        return Err(Error::new("invalid block size in streaminfo"));
    }
    // 20 bits of sample rate, 3 of (channels - 1), 5 of (bits per sample - 1),
    // then 36 bits of total samples. This matches the layout claxon reads.
    let sample_rate_msb = u16::from_be_bytes([b[10], b[11]]);
    let sample_rate_lsb = b[12];
    let sample_rate = (u32::from(sample_rate_msb) << 4) | u32::from(sample_rate_lsb >> 4);
    if sample_rate == 0 || sample_rate > 655_350 {
        return Err(Error::new(format!("invalid sample rate {sample_rate}")));
    }
    let channels = u32::from((sample_rate_lsb >> 1) & 0b111) + 1;
    let bps_lsb = b[13];
    let bits_per_sample = u32::from(((sample_rate_lsb & 1) << 4) | (bps_lsb >> 4)) + 1;
    let samples = (u64::from(bps_lsb & 0b1111) << 32)
        | u64::from(u32::from_be_bytes([b[14], b[15], b[16], b[17]]));
    Ok(StreamBits {
        sample_rate,
        channels,
        bits_per_sample,
        samples: if samples == 0 { None } else { Some(samples) },
    })
}

fn parse_vorbis(data: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if data.len() < 8 {
        return out;
    }
    let vendor_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    if vendor_len > data.len().saturating_sub(8) {
        return out;
    }
    let mut i = 4 + vendor_len;
    if i + 4 > data.len() {
        return out;
    }
    let count = u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
    i += 4;
    for _ in 0..count {
        if i + 4 > data.len() {
            break;
        }
        let len = u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if len > data.len().saturating_sub(i) {
            break;
        }
        let bytes = &data[i..i + len];
        i += len;
        let Some(eq) = bytes.iter().position(|b| *b == b'=') else {
            continue;
        };
        let name = &bytes[..eq];
        if name.is_empty() || name.iter().any(|b| !is_vorbis_name(*b)) {
            continue;
        }
        let name = String::from_utf8_lossy(name).into_owned();
        let value = String::from_utf8_lossy(&bytes[eq + 1..]).into_owned();
        out.push((name, value));
    }
    out
}

fn is_vorbis_name(byte: u8) -> bool {
    (0x20..=0x7d).contains(&byte) && byte != b'='
}

fn parse_cuesheet(data: &[u8]) -> Result<CueSheet> {
    let mut cur = Cursor::new(data);
    // Catalog (128), lead-in (8), flags (1), reserved (258), track count (1).
    cur.skip(128)?;
    let _lead_in = cur.u64_be()?;
    cur.skip(1)?; // CD flag and reserved bits
    cur.skip(258)?;
    let num_tracks = cur.u8()? as usize;

    let mut tracks = Vec::new();
    let mut lead_out_samples = None;
    for _ in 0..num_tracks {
        let offset = cur.u64_be()?;
        let number = cur.u8()?;
        cur.skip(12)?; // ISRC
        cur.skip(1)?; // type and pre-emphasis
        cur.skip(13)?; // reserved
        let num_indices = cur.u8()? as usize;
        let mut indices = Vec::with_capacity(num_indices);
        for _ in 0..num_indices {
            let index_offset = cur.u64_be()?;
            let index_number = cur.u8()?;
            cur.skip(3)?;
            indices.push((index_number, offset.saturating_add(index_offset)));
        }
        // CD lead-out is track 170. Non-CD cuesheets use 255.
        if number == 170 || number == 255 {
            lead_out_samples = Some(offset);
        } else {
            tracks.push(CueTrack { number, indices });
        }
    }
    Ok(CueSheet {
        tracks,
        lead_out_samples,
    })
}

struct Cursor<'a> {
    data: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, i: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .i
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| Error::new("truncated cuesheet block"))?;
        let slice = &self.data[self.i..end];
        self.i = end;
        Ok(slice)
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n)?;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u64_be(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_be_bytes(bytes.try_into().unwrap()))
    }
}

fn unescape_newlines(value: &str) -> String {
    if value.contains(['\n', '\r']) {
        return value.replace("\r\n", "\n").replace('\r', "\n");
    }
    value
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\r", "\n")
}

fn looks_like_cue(text: &str) -> bool {
    text.lines().any(|line| {
        matches!(
            line.split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
                .as_str(),
            "TRACK" | "FILE" | "INDEX"
        )
    })
}

fn expect_flac<R: Read + Seek>(input: &mut R) -> Result<()> {
    let magic = read_array::<_, 4>(input)?;
    if &magic == b"fLaC" {
        return Ok(());
    }
    if &magic[..3] == b"ID3" {
        // `magic` already consumed "ID3" and the major version. Six bytes remain
        // in the 10-byte ID3v2 header: minor, flags, and a 4-byte synchsafe size.
        let rest = read_array::<_, 6>(input)?;
        let size = synchsafe(&rest[2..6]);
        discard(input, u64::from(size))?;
        let magic = read_array::<_, 4>(input)?;
        if &magic == b"fLaC" {
            return Ok(());
        }
    }
    Err(Error::new("not a FLAC file"))
}

fn synchsafe(bytes: &[u8]) -> u32 {
    ((u32::from(bytes[0]) & 0x7f) << 21)
        | ((u32::from(bytes[1]) & 0x7f) << 14)
        | ((u32::from(bytes[2]) & 0x7f) << 7)
        | (u32::from(bytes[3]) & 0x7f)
}

fn read_array<R: Read, const N: usize>(input: &mut R) -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    input
        .read_exact(&mut buf)
        .map_err(|_| Error::new("truncated FLAC file"))?;
    Ok(buf)
}

fn read_vec<R: Read>(input: &mut R, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    input
        .read_exact(&mut buf)
        .map_err(|_| Error::new("truncated FLAC file"))?;
    Ok(buf)
}

fn discard<R: Read + Seek>(input: &mut R, n: u64) -> Result<()> {
    if n == 0 {
        return Ok(());
    }
    if input.seek(SeekFrom::Current(n as i64)).is_ok() {
        return Ok(());
    }
    let mut left = n;
    let mut buf = [0u8; 8192];
    while left > 0 {
        let chunk = usize::try_from(left.min(buf.len() as u64)).unwrap();
        let got = input.read(&mut buf[..chunk])?;
        if got == 0 {
            return Err(Error::new("truncated FLAC file"));
        }
        left -= got as u64;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn streaminfo_payload(rate: u32, channels: u32, bps: u32, samples: u64) -> [u8; 34] {
    let mut bits = Vec::with_capacity(272);
    let mut push = |value: u64, width: u32| {
        for shift in (0..width).rev() {
            let bit = shift < 64 && ((value >> shift) & 1) == 1;
            bits.push(bit);
        }
    };
    push(4096, 16);
    push(4096, 16);
    push(0, 24);
    push(0, 24);
    push(u64::from(rate), 20);
    push(u64::from(channels - 1), 3);
    push(u64::from(bps - 1), 5);
    push(samples, 36);
    push(0, 128);
    let mut out = [0u8; 34];
    for (i, bit) in bits.iter().enumerate() {
        if *bit {
            out[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    out
}

#[cfg(test)]
pub(crate) fn streaminfo_only() -> Vec<u8> {
    let info = streaminfo_payload(44_100, 2, 16, 44_100);
    wrap_blocks(&[(0, &info)])
}

#[cfg(test)]
pub(crate) fn wrap_blocks(parts: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    for (i, (kind, payload)) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let mut head = kind & 0x7f;
        if last {
            head |= 0x80;
        }
        let len = payload.len() as u32;
        out.push(head);
        out.push((len >> 16) as u8);
        out.push((len >> 8) as u8);
        out.push(len as u8);
        out.extend_from_slice(payload);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn streaminfo_matches_the_hand_decoded_notes() {
        // The eight bytes at STREAMINFO offset 10, copied from a real file:
        // 44100 Hz, stereo, 16-bit, 195448260 samples.
        let mut data = [0u8; 34];
        data[0] = 0x10;
        data[1] = 0x00;
        data[2] = 0x10;
        data[3] = 0x00;
        data[10..18].copy_from_slice(&[0x0A, 0xC4, 0x42, 0xF0, 0x0B, 0xA6, 0x4D, 0xC4]);
        let info = parse_streaminfo(&data).unwrap();
        assert_eq!(info.sample_rate, 44_100);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bits_per_sample, 16);
        assert_eq!(info.samples, Some(195_448_260));
    }

    #[test]
    fn packed_streaminfo_round_trips() {
        let payload = streaminfo_payload(48_000, 1, 24, 96_000);
        let info = parse_streaminfo(&payload).unwrap();
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 1);
        assert_eq!(info.bits_per_sample, 24);
        assert_eq!(info.samples, Some(96_000));
    }

    #[test]
    fn cuesheet_block_tracks_and_lead_out() {
        let mut data = vec![0u8; 396];
        data[136] = 0x80; // CD flag. Byte 136 follows the 128-byte catalog and 8-byte lead-in.
        data[395] = 2;
        // One audio track, number 1, offset 0, index 1 at the start of the track.
        data.extend_from_slice(&0u64.to_be_bytes());
        data.push(1);
        data.extend_from_slice(&[0u8; 12]);
        data.push(0);
        data.extend_from_slice(&[0u8; 13]);
        data.push(1);
        data.extend_from_slice(&0u64.to_be_bytes());
        data.push(1);
        data.extend_from_slice(&[0u8; 3]);
        // Lead-out, track 170, at sample 1000, no index points.
        data.extend_from_slice(&1000u64.to_be_bytes());
        data.push(170);
        data.extend_from_slice(&[0u8; 12]);
        data.push(0);
        data.extend_from_slice(&[0u8; 13]);
        data.push(0);

        let sheet = parse_cuesheet(&data).unwrap();
        assert_eq!(sheet.tracks.len(), 1);
        assert_eq!(sheet.tracks[0].number, 1);
        assert_eq!(sheet.tracks[0].indices, vec![(1, 0)]);
        assert_eq!(sheet.lead_out_samples, Some(1000));
    }

    #[test]
    fn index_offsets_are_added_to_the_track_offset() {
        let mut data = vec![0u8; 396];
        data[395] = 1;
        data.extend_from_slice(&500u64.to_be_bytes());
        data.push(2);
        data.extend_from_slice(&[0u8; 12 + 1 + 13]);
        data.push(2);
        data.extend_from_slice(&0u64.to_be_bytes());
        data.push(0);
        data.extend_from_slice(&[0u8; 3]);
        data.extend_from_slice(&80u64.to_be_bytes());
        data.push(1);
        data.extend_from_slice(&[0u8; 3]);
        let sheet = parse_cuesheet(&data).unwrap();
        assert_eq!(sheet.tracks[0].indices, vec![(0, 500), (1, 580)]);
    }

    #[test]
    fn comment_cuesheet_and_id3_prefix() {
        let info = streaminfo_payload(44_100, 2, 16, 44_100);
        let comment = vorbis_block(&[("ARTIST", "Ada"), ("CUE", "FILE \"a.flac\" WAVE\\n  TRACK 01 AUDIO\\n    TITLE \"One\"\\n    INDEX 01 00:00:00\\n")]);
        let bytes = wrap_blocks(&[(0, &info), (4, &comment)]);
        let mut prefixed = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
        prefixed.extend_from_slice(&bytes);
        let meta = read_flac_reader(&mut Cursor::new(&prefixed)).unwrap();
        assert_eq!(meta.sample_rate, 44_100);
        assert_eq!(meta.tag("artist"), Some("Ada"));
        let text = meta.cue_text().unwrap();
        assert!(text.contains("TRACK 01"));
        assert!(text.contains('\n'));
    }

    fn vorbis_block(tags: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        let vendor = b"cuetracks";
        out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        out.extend_from_slice(vendor);
        out.extend_from_slice(&(tags.len() as u32).to_le_bytes());
        for (name, value) in tags {
            let comment = format!("{name}={value}");
            out.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            out.extend_from_slice(comment.as_bytes());
        }
        out
    }

    #[test]
    fn rejects_a_plain_text_file() {
        let err = read_flac_reader(&mut Cursor::new(b"not flac")).unwrap_err();
        assert_eq!(err.to_string(), "not a FLAC file");
    }
}
