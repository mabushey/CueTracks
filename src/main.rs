use clap::Parser;
use colored::*;
use std::{fs, path};
use std::process::exit;
use std::ffi::OsStr;
use walkdir::WalkDir;
use cue_sheet::errors::Error;
use cue_sheet::tracklist::Tracklist;

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
    println!("Entry: {:?}", entry);

    let cue_sheet = fs::read_to_string(entry).unwrap();
    //println!("{}", cue_sheet);
    let mut tracklist = Tracklist::parse(&cue_sheet)?; 

    println!("Debug print of tracklist: {:#?}", tracklist);
    let file = tracklist.files.remove(0);
    //println!("Debug print of file: {:#?}", file);


    let cue_info = format!("{} - {} - {}",
      tracklist.performer.unwrap_or("[No Artist]".to_string()),
      tracklist.date.unwrap_or("[No Year]".to_string()),
      tracklist.title.unwrap_or("[No Album]".to_string())
    );

    if let Some(cd) = tracklist.discnumber {
      if let Some (num) = tracklist.totaldiscs {
        println!(" (CD {}/{})", cd, num);
      } else {
        println!(" (CD {})", cd);
      }
    }

    let dir_info = entry.parent().unwrap().iter().last().unwrap().to_str().unwrap();
    if dir_info == cue_info {
      println!("{}", cue_info.bold());
    } else {
      println!("Data from dir: {}", dir_info);
      println!("Data from cue: {}", cue_info);
    }


    for ref t in file.tracks {

      //  Need to retreive index 1 (1, time)
      //  Could be [0] or [1]
      //  This is ugly, could probably be done with a .map()
      let (a,b) = &t.index[0];
      //println!("a - b: {:?} {:?}", a, b);
      let time = if *a == 1 {
        b
      } else {
        let (_,b) = &t.index[1];
        b
      };

      //println!("Time Index 1: {:?}", time);

      let duration = match t.duration.clone() {
        Some(time) => time.to_string_tiny(),
        None => "??:??".to_string(),
      };
      println!(
          "{:02} - [{}] {} [{}]",
          t.number,
          time.to_string_3(), 
          t.title.as_ref().unwrap(),
          duration
      );
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
