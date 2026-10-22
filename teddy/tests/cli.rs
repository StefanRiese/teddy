//! Runs the teddy binary and compares its output with the original Teddy.exe
//! (tests/fixtures/reference, see tests/fixtures/generate_reference.ps1).

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures").canonicalize().unwrap()
}

/// Runs teddy in the fixtures directory without the online tonies.json.
fn teddy(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_teddy"))
        .current_dir(fixtures())
        .args(["-j", "none.json"])
        .args(args)
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8(out.stdout).unwrap())
}

fn reference(path: &str) -> String {
    std::fs::read_to_string(fixtures().join("reference").join(path)).unwrap()
}

/// (case, uid directory)
const CASES: &[(&str, &str)] = &[
    ("album_cbr64", "0A0B0C03"),
    ("album_cbr96", "0A0B0C01"),
    ("album_prefix", "0A0B0C04"),
    ("album_vbr96", "0A0B0C02"),
    ("files_order", "0A0B0C08"),
    ("mono22k", "0A0B0C05"),
    ("opus_input", "0A0B0C07"),
    ("silence", "0A0B0C06"),
];

#[test]
fn info_output_matches_reference() {
    for (case, dir) in CASES {
        let tonie = format!("reference/{case}/{dir}/500304E0");

        let (ok, text) = teddy(&["-m", "info", &tonie]);
        assert!(ok);
        assert_eq!(text, reference(&format!("{case}/info.txt")), "{case}");

        let (_, csv) = teddy(&["-m", "info", "-f", "csv", &tonie]);
        assert_eq!(csv, reference(&format!("{case}/info.csv")), "{case}");

        /* the original puts a separator before the first entry, which is not valid JSON */
        let (_, json) = teddy(&["-m", "info", "-f", "json", &tonie]);
        assert_eq!(json, reference(&format!("{case}/info.json")).replacen("[\n,\n", "[\n", 1), "{case}");
        serde_json::from_str::<serde_json::Value>(&json).unwrap();
    }
}

#[test]
fn info_of_directory_matches_reference() {
    let (ok, csv) = teddy(&["-m", "info", "-f", "csv", "reference"]);
    assert!(ok);
    assert_eq!(csv, reference("info_all.csv"));
}

#[test]
fn decode_writes_same_files_as_reference() {
    let out = tempfile::tempdir().unwrap();
    for (case, dir) in CASES {
        let tonie = format!("reference/{case}/{dir}/500304E0");
        let case_out = out.path().join(case);
        std::fs::create_dir(&case_out).unwrap();

        let (ok, log) = teddy(&["-m", "decode", "-o", case_out.to_str().unwrap(), &tonie]);
        assert!(ok);
        assert_eq!(log, reference(&format!("{case}/decode.log")), "{case}");

        let mut ours: Vec<String> = std::fs::read_dir(&case_out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        let mut theirs: Vec<String> = std::fs::read_dir(fixtures().join("reference").join(case).join("decode"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        ours.sort();
        theirs.sort();
        assert_eq!(ours, theirs, "{case}");

        let (_, log) = teddy(&["-m", "decode", "-s", "-o", case_out.to_str().unwrap(), &tonie]);
        assert_eq!(log, reference(&format!("{case}/decode_single.log")), "{case}");
    }
}

/// The encode log without the progress inside the brackets, which depends on the decoder's read size.
fn without_progress(log: &str) -> String {
    log.lines()
        .map(|l| match (l.find('['), l.rfind(']')) {
            (Some(a), Some(b)) if l.starts_with(" Track") && a < b => format!("{}[]", &l[..a]),
            _ => l.to_string(),
        })
        /* page layout warnings depend on the exact packet sizes */
        .filter(|l| !l.starts_with("[WARNING] Hit block boundary"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn encode_output_matches_reference() {
    let out = tempfile::tempdir().unwrap();
    /* files given explicitly keep their order; see directories_are_sorted_by_id3_track_number for folders */
    let runs: &[(&str, &[&str])] = &[
        ("mono22k", &["-i", "0x0E03421A", "inputs/mono22k.mp3"]),
        ("files_order", &["-i", "0x5E03421D", "inputs/album/c_end.mp3", "inputs/mono22k.mp3"]),
        ("opus_input", &["-i", "0x5E03421C", "-b", "48", "inputs/opus_input.ogg"]),
    ];
    for (case, args) in runs {
        let dir = CASES.iter().find(|(c, _)| c == case).unwrap().1;
        let target = format!("reference/{case}/{dir}/500304E0");
        let out_file = out.path().join(case);
        let mut all = vec!["-m", "encode", "-o", out_file.to_str().unwrap()];
        all.extend_from_slice(args);

        let (ok, log) = teddy(&all);
        assert!(ok, "{case}: {log}");
        /* the original keeps the NUL terminator of ID3 titles */
        let expected = reference(&format!("{case}/encode.log"))
            .replace(&target, out_file.to_str().unwrap())
            .replace('\0', " ");
        assert_eq!(without_progress(&log), without_progress(&expected), "{case}");
        assert!(out_file.is_file());
    }
}

#[test]
fn encode_fails_for_missing_files() {
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("500304E0");
    let (ok, log) = teddy(&["-m", "encode", "-o", target.to_str().unwrap(), "inputs/missing.mp3"]);
    assert!(!ok);
    assert!(log.contains("[ERROR] Failed to process due to a missing file"), "{log}");
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0, "partial output left behind");
}
