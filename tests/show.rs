use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cuetracks"))
        .args(args)
        .output()
        .expect("run cuetracks")
}

#[test]
fn sample_cues_list_titles_without_the_old_debug_output() {
    let output = run(&[
        "tests/Clepsydra - Hologram.flac.cue",
        "tests/Impellitteri - System X.cue",
        "tests/Marillion - Misplaced Childhood (CD2).flac.cue",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Clepsydra — Hologram"));
    assert!(stdout.contains("Sunrise"));
    assert!(stdout.contains("United We Stand"));
    assert!(stdout.contains("Lady Nina"));
    assert!(stdout.contains("CD 2/2"));
    assert!(stdout.contains("White Feather (Album Demo)"));
    assert!(!stdout.contains("Data from"));
    assert!(!stdout.contains("Flac specs"));
}

#[test]
fn a_missing_path_fails() {
    let output = run(&["no-such-album.cue"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no such path"));
    assert!(stderr.contains("no cue sheets found"));
}
