//! Compares tonie-core against files produced by the original C# Teddy.exe
//! (tests/fixtures/reference, see tests/fixtures/generate_reference.ps1).

use std::io::Cursor;
use std::path::{Path, PathBuf};

use tonie_core::encode::{encode, EncodeOptions};
use tonie_core::header::BLOCK_SIZE;
use tonie_core::info::{dump_info, DumpFormat};
use tonie_core::ogg::Page;
use tonie_core::writer::{FrameEncoder, CHANNELS, FRAME_SAMPLES};
use tonie_core::{Error, TonieFile};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures").canonicalize().unwrap()
}

struct Case {
    name: String,
    /// Path of the Tonie file relative to the fixtures directory, as passed to Teddy.exe
    tonie: String,
}

impl Case {
    fn dir(&self) -> PathBuf {
        fixtures().join("reference").join(&self.name)
    }

    fn file(&self) -> PathBuf {
        fixtures().join(&self.tonie)
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir().join(name)).unwrap()
    }
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for entry in std::fs::read_dir(fixtures().join("reference")).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        for sub in std::fs::read_dir(&dir).unwrap() {
            let sub = sub.unwrap().path();
            if sub.join("500304E0").is_file() {
                let uid_dir = sub.file_name().unwrap().to_string_lossy().to_string();
                cases.push(Case {
                    tonie: format!("reference/{name}/{uid_dir}/500304E0"),
                    name: name.clone(),
                });
            }
        }
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    assert!(cases.len() >= 8, "reference files missing");
    cases
}

/// Runs `f` with the fixtures directory as working directory, so relative paths match the reference output.
fn in_fixtures<T>(f: impl FnOnce() -> T) -> T {
    /* every test uses the same directory, so running them in parallel is fine */
    std::env::set_current_dir(fixtures()).unwrap();
    f()
}

#[test]
fn reference_files_are_valid() {
    for case in cases() {
        let file = TonieFile::open(case.file(), true).unwrap();
        assert!(file.hash_correct, "{}", case.name);
        assert_eq!(file.header_length, 0xFFC, "{}", case.name);
        assert_eq!(file.header.audio_length as usize, file.audio.len(), "{}", case.name);
        assert!(file.statistics.errors.is_empty(), "{}: {:?}", case.name, file.statistics.errors);
    }
}

#[test]
fn info_matches_reference() {
    in_fixtures(|| {
        for case in cases() {
            let path = Path::new(&case.tonie);

            let mut text = String::new();
            dump_info(&mut text, DumpFormat::Text, path, &mut [], None, true).unwrap();
            let expected = case.read("info.txt");
            assert_eq!(text, expected.strip_prefix("[Mode: dump information]\n").unwrap(), "{}", case.name);

            let mut csv = String::new();
            dump_info(&mut csv, DumpFormat::Csv, path, &mut [], None, true).unwrap();
            assert_eq!(csv, case.read("info.csv"), "{}", case.name);

            let mut json = String::new();
            dump_info(&mut json, DumpFormat::Json, path, &mut [], None, true).unwrap();
            /* the original writes a separator before the first entry, which makes the JSON invalid */
            let expected = case.read("info.json");
            let expected = expected.strip_prefix("[\n,\n").unwrap().strip_suffix("]\n").unwrap();
            assert_eq!(json, expected, "{}", case.name);
            serde_json::from_str::<serde_json::Value>(&format!("[{json}]")).unwrap();
        }
    });
}

/// Packets of the OpusTags header with the comment values that depend on the tool version blanked.
fn normalized_tags(packet: &[u8]) -> Vec<String> {
    let vendor_len = u32::from_le_bytes(packet[8..12].try_into().unwrap()) as usize;
    let mut pos = 12 + vendor_len;
    let count = u32::from_le_bytes(packet[pos..pos + 4].try_into().unwrap());
    pos += 4;
    let mut tags = vec![String::from_utf8_lossy(&packet[12..12 + vendor_len]).to_string()];
    for _ in 0..count {
        let len = u32::from_le_bytes(packet[pos..pos + 4].try_into().unwrap()) as usize;
        let tag = String::from_utf8_lossy(&packet[pos + 4..pos + 4 + len]).to_string();
        tags.push(if tag.starts_with("TeddyVersion=") { "TeddyVersion=".into() } else { tag });
        pos += 4 + len;
    }
    tags
}

fn pages(data: &[u8]) -> Vec<Page> {
    let mut pages = Vec::new();
    let mut offset = 0;
    while let Some((page, size)) = Page::parse(data, offset) {
        pages.push(page);
        offset += size;
    }
    assert_eq!(offset, data.len(), "trailing data after the last Ogg page");
    pages
}

#[test]
fn decode_matches_reference() {
    in_fixtures(|| {
        let out_dir = tempfile::tempdir().unwrap();
        for case in cases() {
            let file = TonieFile::open(&case.tonie, true).unwrap();
            let hash = file.hash_string();
            let tags = vec!["TeddyVersion=x".to_string(), format!("TeddyFile={}", case.tonie), format!("HASH={hash}")];
            let name = format!("500304E0-{:08X}", file.header.audio_id);

            let single = file.dump_audio_files(out_dir.path(), &name, true, &tags, None).unwrap();
            let expected = std::fs::read(case.dir().join("decode_single").join(format!("{name}.ogg"))).unwrap();
            assert!(std::fs::read(&single[0]).unwrap() == expected, "{}: single file differs", case.name);

            let tracks = file.dump_audio_files(out_dir.path(), &name, false, &tags, None).unwrap();
            assert_eq!(tracks.len(), file.header.audio_chapters.len());
            for track in tracks {
                let file_name = track.file_name().unwrap();
                let ours = pages(&std::fs::read(&track).unwrap());
                let theirs = pages(&std::fs::read(case.dir().join("decode").join(file_name)).unwrap());
                assert_eq!(ours.len(), theirs.len(), "{}: {file_name:?}", case.name);
                for (a, b) in ours.iter().zip(&theirs) {
                    if a.packet_magic() == Some(b"OpusTags") {
                        assert_eq!(normalized_tags(&a.segments[0]), normalized_tags(&b.segments[0]), "{}: {file_name:?}", case.name);
                    } else {
                        assert_eq!(a, b, "{}: {file_name:?} page {}", case.name, b.sequence);
                    }
                }
            }
        }
    });
}

/// Returns the packets of an existing file, so the page layout can be checked byte by byte
/// independently of the Opus encoder version.
struct ReplayEncoder {
    packets: std::vec::IntoIter<Vec<u8>>,
}

impl FrameEncoder for ReplayEncoder {
    fn encode(&mut self, _pcm: &[i16], out: &mut [u8]) -> Result<usize, Error> {
        let packet = self.packets.next().ok_or_else(|| Error::Encoder("out of packets".into()))?;
        if packet.len() > out.len() {
            return Err(Error::Encoder(format!(
                "packet of {} bytes does not fit into {} bytes",
                packet.len(),
                out.len()
            )));
        }
        out[..packet.len()].copy_from_slice(&packet);
        Ok(packet.len())
    }
}

fn audio_packets(audio: &[u8]) -> Vec<Vec<u8>> {
    pages(audio).into_iter().filter(|p| !p.is_opus_header()).flat_map(|p| p.segments).collect()
}

#[test]
fn page_layout_matches_reference() {
    for case in cases() {
        let reference = TonieFile::open(case.file(), true).unwrap();
        let packets = audio_packets(&reference.audio);

        /* finishing the stream adds one or two frames of silence */
        let matched = [1, 2].iter().any(|finish_frames| {
            let encoder = ReplayEncoder {
                packets: packets.clone().into_iter(),
            };
            let mut writer =
                tonie_core::writer::TonieOggWriter::new(encoder, Vec::new(), &tonie_core::writer::OpusTags::teddy(), reference.header.audio_id).unwrap();
            let frame = vec![1i16; FRAME_SAMPLES * CHANNELS];
            for _ in 0..packets.len() - finish_frames {
                if writer.write_samples(&frame).is_err() {
                    return false;
                }
            }
            matches!(writer.finish(), Ok((out, _)) if out == reference.audio)
        });
        assert!(matched, "{}: page layout differs from the reference", case.name);
    }
}

/// Header of a Tonie file encoded by the Rust port from the same inputs.
fn encode_case(sources: &[&str], options: EncodeOptions) -> TonieFile {
    let files: Vec<PathBuf> = sources.iter().map(|s| fixtures().join(s)).collect();
    let files = tonie_core::build_file_list(&files, &mut ()).unwrap();
    let mut out = Cursor::new(Vec::new());
    encode(&files, &options, &mut out, &mut ()).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("500304E0");
    std::fs::write(&path, out.into_inner()).unwrap();
    TonieFile::open(&path, true).unwrap()
}

#[test]
fn encoding_matches_reference_structure() {
    let opts = |id, bitrate, vbr, prefix: Option<&str>| EncodeOptions {
        audio_id: id,
        bitrate,
        vbr,
        prefix_dir: prefix.map(|p| fixtures().join(p)),
    };
    /* Teddy.exe sorts directories by ID3 track number, but cannot read the NUL terminated numbers ffmpeg writes,
     * so it used the file name order. Pass the files explicitly in that order. */
    let album = vec!["inputs/album/a_intro.mp3", "inputs/album/b_song.mp3", "inputs/album/c_end.mp3"];
    let runs: Vec<(&str, Vec<&str>, EncodeOptions)> = vec![
        ("album_cbr96", album.clone(), opts(0x5E03_4216, 96000, false, None)),
        ("album_vbr96", album.clone(), opts(0x5E03_4217, 96000, true, None)),
        ("album_cbr64", album.clone(), opts(0x5E03_4218, 64000, false, None)),
        ("album_prefix", album.clone(), opts(0x5E03_4219, 96000, false, Some("inputs/prefix"))),
        ("mono22k", vec!["inputs/mono22k.mp3"], opts(0x0E03_421A, 96000, false, None)),
        ("silence", vec!["inputs/silence.mp3"], opts(0x5E03_421B, 96000, false, None)),
        ("opus_input", vec!["inputs/opus_input.ogg"], opts(0x5E03_421C, 48000, false, None)),
        (
            "files_order",
            vec!["inputs/album/c_end.mp3", "inputs/mono22k.mp3"],
            opts(0x5E03_421D, 96000, false, None),
        ),
    ];

    for (name, sources, options) in runs {
        let cbr = !options.vbr;
        let has_prefix = options.prefix_dir.is_some();
        let sources_count = sources.len();
        let ours = encode_case(&sources, options);
        let theirs = TonieFile::open(cases().iter().find(|c| c.name == name).unwrap().file(), true).unwrap();
        let (a, b) = (&ours.statistics, &theirs.statistics);

        assert!(ours.hash_correct, "{name}");
        assert_eq!(ours.header_length, 0xFFC, "{name}");
        assert!(a.errors.is_empty(), "{name}: {:?}", a.errors);
        assert_eq!(ours.header.audio_id, theirs.header.audio_id, "{name}");
        assert!(!ours.audio.len().is_multiple_of(BLOCK_SIZE), "{name}: file ends at a block boundary");
        assert_eq!(ours.header.audio_chapters.len(), theirs.header.audio_chapters.len(), "{name}");

        /* Teddy.exe passes a full 60 ms buffer to the encoder after the last read of every file, repeating up to
         * 3 frames of stale audio per decoded file. The port does not, so it may only be shorter by that much. */
        let decoded_files = sources_count * if has_prefix { 2 } else { 1 };
        let tolerance = (3 * decoded_files as u64 + 1) * FRAME_SAMPLES as u64;
        assert!(
            a.highest_granule <= b.highest_granule && b.highest_granule - a.highest_granule <= tolerance,
            "{name}: duration {} vs {}",
            a.highest_granule,
            b.highest_granule
        );
        for (x, y) in ours.header.audio_chapters.iter().zip(&theirs.header.audio_chapters) {
            assert!(
                x.abs_diff(*y) <= 1,
                "{name}: chapters {:?} vs {:?}",
                ours.header.audio_chapters,
                theirs.header.audio_chapters
            );
        }
        if cbr {
            assert_eq!((a.min_segments, a.max_segments), (b.min_segments, b.max_segments), "{name}");
            assert_eq!((a.min_granule, a.max_granule), (b.min_granule, b.max_granule), "{name}");
        }

        /* every packet must decode */
        let mut decoder = opus::Decoder::new(48000, opus::Channels::Stereo).unwrap();
        let mut pcm = vec![0i16; 5760 * 2];
        for packet in audio_packets(&ours.audio) {
            decoder.decode(&packet, &mut pcm, false).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}

#[test]
fn directories_are_sorted_by_id3_track_number() {
    let files = tonie_core::build_file_list(&[fixtures().join("inputs/album")], &mut ()).unwrap();
    let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().to_string()).collect();
    assert_eq!(names, ["b_song.mp3", "a_intro.mp3", "c_end.mp3"]);
}
