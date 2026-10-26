//! Show track names and durations from cue sheets and FLAC files.

#![forbid(unsafe_code)]

mod album;
mod cue;
mod error;
mod flac;

use album::{album_from_cue, album_from_flac, render, FileAudio};
use clap::Parser;
use cue::{parse_cue, read_sheet};
use error::{Error, Result};
use flac::read_flac;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use walkdir::WalkDir;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Cue sheets, FLAC files, or directories to scan
    #[arg(required = true, num_args = 1.., value_name = "PATH")]
    paths: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
struct Found {
    path: PathBuf,
    explicit: bool,
}

enum Source {
    Cue(PathBuf),
    Flac(PathBuf),
}

impl Source {
    fn path(&self) -> &Path {
        match self {
            Source::Cue(path) | Source::Flac(path) => path,
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let color = std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal();
    match run(&args.paths, color) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(1),
    }
}

fn run(paths: &[PathBuf], color: bool) -> std::result::Result<(), ()> {
    let (sources, mut failed) = gather(paths);
    if sources.is_empty() {
        eprintln!("no cue sheets found");
        return Err(());
    }
    let show_source = sources.len() > 1;
    let mut printed = false;
    for source in &sources {
        match load(source) {
            Ok(album) => {
                if printed {
                    println!();
                }
                print!("{}", render(&album, color, show_source));
                printed = true;
            }
            Err(err) => {
                eprintln!("{}: {err}", source.path().display());
                failed = true;
            }
        }
    }
    if failed {
        Err(())
    } else {
        Ok(())
    }
}

fn gather(paths: &[PathBuf]) -> (Vec<Source>, bool) {
    let mut cues: Vec<Found> = Vec::new();
    let mut flacs: Vec<Found> = Vec::new();
    let mut failed = false;

    for path in paths {
        if !path.exists() {
            eprintln!("no such path: {}", path.display());
            failed = true;
            continue;
        }
        if path.is_dir() {
            for entry in WalkDir::new(path) {
                match entry {
                    Ok(entry) if entry.file_type().is_file() => {
                        push_media(&mut cues, &mut flacs, entry.path(), false);
                    }
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!("warning: {err}");
                        failed = true;
                    }
                }
            }
        } else if path.is_file() {
            if !push_media(&mut cues, &mut flacs, path, true) {
                eprintln!("unsupported file: {}", path.display());
                failed = true;
            }
        }
    }

    dedup(&mut cues);
    dedup(&mut flacs);

    let mut referenced = HashSet::new();
    for cue in &cues {
        if let Ok(text) = read_sheet(&cue.path) {
            if let Ok(sheet) = parse_cue(&text) {
                for file in &sheet.files {
                    if let Some(audio) = resolve_audio(&cue.path, &file.name) {
                        referenced.insert(identity(&audio));
                    }
                }
            }
        }
    }

    let cue_paths: Vec<PathBuf> = cues.iter().map(|cue| cue.path.clone()).collect();
    let mut sources: Vec<Source> = cues.into_iter().map(|cue| Source::Cue(cue.path)).collect();
    for flac in flacs {
        let id = identity(&flac.path);
        let linked = referenced.contains(&id) || sidecar_is_listed(&flac.path, &cue_paths);
        if linked {
            continue;
        }
        if !flac.explicit && !has_embedded_cue(&flac.path) {
            continue;
        }
        sources.push(Source::Flac(flac.path));
    }

    sources.sort_by(|a, b| {
        a.path()
            .to_string_lossy()
            .to_lowercase()
            .cmp(&b.path().to_string_lossy().to_lowercase())
    });
    let mut seen = HashSet::new();
    sources.retain(|source| seen.insert(identity(source.path())));
    (sources, failed)
}

fn push_media(cues: &mut Vec<Found>, flacs: &mut Vec<Found>, path: &Path, explicit: bool) -> bool {
    let ext = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "cue" => cues.push(Found {
            path: path.to_path_buf(),
            explicit,
        }),
        "flac" => flacs.push(Found {
            path: path.to_path_buf(),
            explicit,
        }),
        _ => return false,
    }
    true
}

fn dedup(found: &mut Vec<Found>) {
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen = HashSet::new();
    found.retain(|item| seen.insert(identity(&item.path)));
}

fn identity(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn sidecar_paths(flac: &Path) -> [PathBuf; 2] {
    let mut named = flac.as_os_str().to_owned();
    named.push(".cue");
    [PathBuf::from(named), flac.with_extension("cue")]
}

fn sidecar_is_listed(flac: &Path, cues: &[PathBuf]) -> bool {
    let candidates = sidecar_paths(flac);
    cues.iter()
        .any(|cue| candidates.iter().any(|candidate| same_file(candidate, cue)))
}

fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn sidecar_cue(flac: &Path) -> Option<PathBuf> {
    sidecar_paths(flac).into_iter().find(|path| path.is_file())
}

fn has_embedded_cue(path: &Path) -> bool {
    read_flac(path).is_ok_and(|meta| meta.has_embedded_cue())
}

fn load(source: &Source) -> Result<album::Album> {
    match source {
        Source::Cue(path) => load_cue(path),
        Source::Flac(path) => load_flac(path),
    }
}

fn load_cue(path: &Path) -> Result<album::Album> {
    let text = read_sheet(path)?;
    let sheet = parse_cue(&text)?;
    let files: Vec<FileAudio> = sheet
        .files
        .iter()
        .map(|file| audio_for(&resolve_audio(path, &file.name)))
        .collect();
    album_from_cue(&display_path(path), &sheet, &files)
}

fn load_flac(path: &Path) -> Result<album::Album> {
    let meta = read_flac(path)?;
    if meta.has_embedded_cue() {
        return album_from_flac(&display_path(path), &meta);
    }
    if let Some(cue) = sidecar_cue(path) {
        return load_cue(&cue);
    }
    Err(Error::new("no cue sheet found in the file or next to it"))
}

fn audio_for(path: &Option<PathBuf>) -> FileAudio {
    let Some(path) = path else {
        return FileAudio::default();
    };
    read_flac(path)
        .map(|meta| FileAudio::from_flac(&meta))
        .unwrap_or_default()
}

fn resolve_audio(cue_path: &Path, file_name: &str) -> Option<PathBuf> {
    let dir = cue_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let named = Path::new(file_name);
    let mut candidates = Vec::new();
    if named.is_absolute() {
        candidates.push(named.to_path_buf());
    } else {
        candidates.push(dir.join(named));
    }
    if let Some(base) = named.file_name() {
        let beside = dir.join(base);
        if !candidates.iter().any(|path| path == &beside) {
            candidates.push(beside);
        }
    }
    let flac_names: Vec<PathBuf> = candidates
        .iter()
        .map(|path| path.with_extension("flac"))
        .collect();
    for extra in flac_names {
        if !candidates.iter().any(|path| path == &extra) {
            candidates.push(extra);
        }
    }
    if let Some(found) = candidates.iter().find(|path| path.is_file()) {
        return Some(found.clone());
    }
    let wanted: Vec<String> = candidates
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
        })
        .collect();
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if wanted.iter().any(|want| want == &name) && entry.path().is_file() {
            return Some(entry.path());
        }
    }
    None
}

fn display_path(path: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return path.display().to_string();
    };
    path.canonicalize()
        .ok()
        .and_then(|abs| abs.strip_prefix(&cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cuetracks-{nanos}-{n}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_directory_lists_the_cue_once_and_skips_the_audio_it_names() {
        let dir = scratch();
        let cue = dir.join("album.flac.cue");
        fs::write(
            &cue,
            "PERFORMER \"A\"\nTITLE \"B\"\nFILE \"album.flac\" WAVE\nTRACK 01 AUDIO\nTITLE \"Song\"\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        fs::write(dir.join("album.flac"), b"not really flac").unwrap();
        fs::write(dir.join("notes.txt"), b"ignore").unwrap();
        let (sources, failed) = gather(&[dir.clone()]);
        assert!(!failed);
        assert_eq!(sources.len(), 1);
        assert!(matches!(&sources[0], Source::Cue(path) if path == &cue));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_explicit_flac_uses_the_sidecar_cue() {
        let dir = scratch();
        fs::write(dir.join("album.cue"), "TITLE \"Side\"\nFILE \"album.flac\" WAVE\nTRACK 01 AUDIO\nTITLE \"Song\"\nINDEX 01 00:00:00\n").unwrap();
        let flac = dir.join("album.flac");
        fs::write(&flac, crate::flac::streaminfo_only()).unwrap();
        let (sources, _) = gather(&[flac]);
        assert_eq!(sources.len(), 1);
        let album = load(&sources[0]).unwrap();
        assert_eq!(album.title.as_deref(), Some("Side"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolves_a_flac_when_the_cue_names_a_wav() {
        let dir = scratch();
        let cue = dir.join("album.cue");
        fs::write(&cue, "").unwrap();
        fs::write(dir.join("album.flac"), b"").unwrap();
        let found = resolve_audio(&cue, "album.wav").unwrap();
        assert_eq!(found.file_name().unwrap(), "album.flac");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_directory_of_plain_flacs_is_not_listed() {
        let dir = scratch();
        fs::write(dir.join("song.flac"), crate::flac::streaminfo_only()).unwrap();
        let (sources, failed) = gather(&[dir.clone()]);
        assert!(!failed);
        assert!(sources.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_explicit_flac_without_a_cue_is_still_opened() {
        let dir = scratch();
        let flac = dir.join("song.flac");
        fs::write(&flac, crate::flac::streaminfo_only()).unwrap();
        let (sources, failed) = gather(&[flac]);
        assert!(!failed);
        assert_eq!(sources.len(), 1);
        assert!(load(&sources[0]).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
