# cuetracks

Show track names and durations from cue sheets and FLAC files.

```
Artist — Album
1985 · CD 2/2 · 16-bit/44.1 kHz stereo · 1:13:52

  #  Title        Start  Length
 01  First Song    0:00    5:47
 02  Second Song   5:50    4:06
```

## Install

Rust 1.99 or newer:

```
cargo build --release
install -m 755 target/release/cuetracks ~/.nix-profile/bin/cuetracks
```

`~/.nix-profile/bin` is on `PATH` for a normal NixOS login. Any other directory on `PATH` works the same way.

The binary is linked against the glibc from the build environment. On NixOS, build it on the machine where you will run it.

A Nix shell with the compiler and `cargo test` tools:

```
nix develop
```

## Usage

```
cuetracks <PATH>...
```

Each path is a `.cue` file, a `.flac` file, or a directory. Directories are scanned for cue sheets and for FLAC files that carry an embedded cue. A FLAC that is named by a cue sheet, or that has a sidecar cue already in the list, is not printed a second time. A FLAC found only by the scan is skipped when it has no embedded cue. Passing that FLAC on the command line still opens it, and then looks for `file.flac.cue` or `file.cue` beside it.

```
cuetracks "Artist - 1989 - Album"
cuetracks album.cue disc.flac
```

Several albums are separated by a blank line, and each one is labeled with its path. The program exits 0 when every sheet prints. It exits 1 when a path is missing, a sheet cannot be read, or nothing to show was found. Sheets that did parse are still printed.

Color is used when stdout is a terminal and `NO_COLOR` is unset.

## What is read

Text cue sheets may be UTF-8 (with or without a BOM), UTF-16 with a BOM, or Windows-1252. Command names are case-insensitive. `REM` tags used for the heading are `DATE` or `YEAR`, `DISCNUMBER` or `DISC`, and `TOTALDISCS` or `DISCTOTAL`. A date of `YYYY-MM-DD` is shown as the year. Other `REM` lines, including ReplayGain values that end in `dB`, are ignored.

A FLAC may carry a binary `CUESHEET` block, or a text cue in a Vorbis comment named `CUESHEET` or `CUE`. Album performer, title, date, and disc number come from the Vorbis comments `ALBUMARTIST`, `ALBUM ARTIST`, `ARTIST`, `PERFORMER`, `ALBUM`, `TITLE`, `DATE`, `YEAR`, `DISCNUMBER`, `DISC`, `TOTALDISCS`, and `DISCTOTAL`. A track whose performer differs from the album performer is shown as `Artist — Title`. A binary cue sheet has no titles, so those rows are `Track NN`.

Audio named by a `FILE` line is found beside the cue sheet. A `.wav` name is also tried as `.flac`. The sample rate, bit depth, and channel count in the subtitle come from the first FLAC that can be opened.

## How lengths are chosen

Cue time is `mm:ss:ff` at 75 frames per second. Minutes may pass 59.

The start of a song is its `INDEX 01`. The printed start drops leftover frames, so `5:50:10` is `5:50`. The length runs from that start to the next song's first index in the same file, and rounds to the nearest second. 38 frames and above round up. When the next song has an `INDEX 00`, that index is the end of this song, and the pregap is not part of either song. `PREGAP` and `POSTGAP` are burner commands and are not added to a length.

Index times belong to the current `FILE`. One track may end with `INDEX 00` in one file and continue with `INDEX 01` at the start of the next file. The earlier song ends at that `INDEX 00`. The continued song starts at `0:00` in the new file and runs to the next index there, or to the end of that file. An index whose time moves backward past a lower-numbered index in the same file is ignored.

The last song in a file uses the audio file's length when that file can be read. A missing start or length is `?`. The album total is the sum of the file lengths when every file has one, so it includes pregaps that are stored in those files.

## Tests

```
cargo test
```

The cue sheets under `tests/` are fixtures for those tests.

## License

cuetracks is licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
