use clap::Parser;
use colored::*;
use std::{fs, path};
use std::process::exit;
use std::ffi::OsStr;
use walkdir::WalkDir;
use cue_sheet::errors::Error;
use cue_sheet::tracklist::Tracklist;
use cue_sheet::parser::Time;
use claxon;
use rounded_div::RoundedDiv;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
  /// Video files to process
  #[arg(required = true, num_args = 1..)]
  paths: Vec<String>,
}

//struct cue {
//}

fn main() -> Result<(), Error> {
  let opt = Args::parse();
  //println!("Args: {:#?}", opt);
  // May specify a dir with multiple cue files. Is this just a starting point?
  let mut cue_files: Vec<path::PathBuf> = Vec::with_capacity(opt.paths.len());
  for path in opt.paths.iter() {
    let path = path::Path::new(path);
    if path.is_dir() {
      //println!("{}: {}", "We Have a directory".green() , path.display());
      for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if entry.path().extension() == Some(OsStr::new("cue")) {
          //println!("Cue: {}", entry.path().display());
          cue_files.push(entry.path().to_path_buf());

        }
      }
      continue;
    }
    if path.is_file() {
      if path.extension() == Some(OsStr::new("cue")) {
        //println!("Got a Cue: {}", path.display());
        cue_files.push(path.to_path_buf());
      } else {
        println!("File is not a .cue: {}", path.display());
        continue;
      }
    } else {
      println!("{}: {}", "Not a file - should never get this message".red() , path.display());
      continue;
    }
  }
  if cue_files.len() < 1 {
    println!("{}", "Exiting, no files to process".red());
    exit(1);
  }

  //file_array.sort_by_key(|x| x.arg_path.clone());

  //println!("Output: {:?}", cue_files);

  for entry in cue_files.iter() {
    println!("Cue Sheet: {:?}", entry);

    let cue_sheet = fs::read_to_string(entry).unwrap();
    //println!("{}", cue_sheet);
    let mut tracklist = Tracklist::parse(&cue_sheet)?; 

    //println!("Debug print of tracklist: {:#?}", tracklist);
    let file = tracklist.files.remove(0);
    //println!("Debug print of file: {:#?}", file);

    let cue_info = format!("{} - {} - {}",
      tracklist.performer.unwrap_or("[No Artist]".to_string()),
      tracklist.date.unwrap_or("[No Year]".to_string()),
      tracklist.title.unwrap_or("[No Album]".to_string())
    );

    let cd_info = {
      if let Some(cd) = tracklist.discnumber {
        if let Some (num) = tracklist.totaldiscs {
          Some(format!("CD{}/{}", cd, num))
        } else {
          Some(format!("CD{}", cd))
        }
      } else {
        None
      }
    };

    let parent_dir = entry.parent().unwrap();
    //println!("parent dir: {:?}", parent_dir);
    let file_flac = path::Path::new(parent_dir).join(file.name);
    //println!("Flac file: {:?}", file_flac);
    let total_seconds = if file_flac.is_file() {
      if file_flac.extension() == Some(OsStr::new("flac")) {
          let reader = claxon::FlacReader::open(&file_flac).expect("failed to open FLAC stream");
          for (name, value) in reader.tags() {
            // Print comments in a format similar to what
            // `metaflac --block-type=VORBIS_COMMENT --list` would print.
            println!("{}={}", name, value);
          }
          let streaminfo = reader.streaminfo();
          let sample_rate:u64 = streaminfo.sample_rate.into();
          println!("Flac specs: {}/{} {} Channels", streaminfo.bits_per_sample, sample_rate/1000, streaminfo.channels);

          if let Some(x) = streaminfo.samples {
            Some(x.rounded_div(sample_rate))
          } else {
            None
          }
      } else {
        None
      }
    } else {
      None
    };
    //println!("Got total: {:?})", total_seconds);

    println!();
    let dir_info = parent_dir.iter().last().unwrap().to_str().unwrap();
    if dir_info == cue_info {
      print!("{}", cue_info.bold());
    } else {
      println!("Data from dir: {}", dir_info);
      print!("Data from cue: {}", cue_info);
    }
    if let Some(x) = cd_info {
      println!(" - {}", x.yellow());
    } else {
      println!();
    }



    for ref t in file.tracks {

      // Retreive INDEX 01 - Could be 1st or 2nd element
      // INDEX 01 is required and denotes the start of the track, while
      // INDEX 00 is optional and denotes the pregap.
      let (_,time) = t.index.clone().into_iter().find(|a| a.0 == 1).unwrap();

      let duration = match t.duration.clone() {
        Some(time) => time.to_string_tiny(),
        None => {
          match total_seconds {
            Some(sec) => {
                let flac_frames:i64 = (sec*75).try_into().unwrap();
                let track_frames = time.total_frames();
                Time::from_frames(flac_frames - track_frames).to_string_tiny()
            },
            None => "??:??".to_string(),
          }
        }
      };
      println!(
          "{:02} - [{}] {} [{}]",
          t.number,
          time.to_string_3(), 
          t.title.as_ref().unwrap(),
          duration
      );
          //println!("Index: {:?}", t.index);
          //let tmp = t.index.clone().into_iter().find(|a| a.0 == 0);
          //println!("Index 0: {:?}", tmp);
          //let tmp = t.index.clone().into_iter().find(|a| a.0 == 1);
          //println!("Index 1: {:?}", tmp);
          //let tmp = t.index.clone().into_iter().nth(0);
          //println!("First Index: {:?}", tmp);

    }

    // Add this back in for VA releases
    //        t.performer
    //            .clone()
    //            .ok_or_else(|| Error::from("Not all tracks have a specified performer."))?,

//    for (index, track) in cd.tracks().iter().enumerate() {
//      //println!("{} - [{}] {:?} [{:?}]", index + 1, track.get_start(), track.get_track(), track.get_length());
//      println!("{} - [{}]  [{:?}]", index + 1, track.get_start(), track.get_length());
//    }

  }
  Ok(())
}
