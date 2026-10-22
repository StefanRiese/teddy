//! Teddy: inspect, decode and encode Toniebox audio files.
//! Port of the original C# Teddy.exe with the same options and output.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgAction, Parser};
use tonie_core::encode::{display_name, EncodeOptions, EncodeProgress};
use tonie_core::info::{dump_info, DumpFormat, CSV_HEADER};
use tonie_core::tonies::{self, TonieData};
use tonie_core::{Error, TonieFile};

const VERSION: &str = env!("TEDDY_VERSION");

#[derive(Parser)]
#[command(
    name = "teddy",
    version = VERSION,
    before_help = concat!(" Tonie Encoder Decoder for DIYs - (c)2020 Team RevvoX\n", "------------------------------------------------------"),
    override_usage = "teddy -m decode [options] <toniefile>          - Dump tonie file content
       teddy -m info [options] <toniefile>            - Show header details
       teddy -m encode [options] <folder>             - Create a tonie file from all audio files in this folder
       teddy -m encode [options] <file1> <file2> ...  - Create a tonie file from specified audio files
       teddy -m rename [options] <source> <dest>      - Rename tonie files by their title",
    after_help = "Hints:
  Prefix files are files named '0001.mp3', '0002.mp3', ..., '9999.mp3' and will get prepended to the
  real track audio data. These are meant to add the track number in front of the file so the
  kids know which track number is played right now.

  As JSON file you could specify also a link like e.g. https://api.revvox.de/tonies.json"
)]
struct Args {
    /// Operating mode: info, decode, encode, rename
    #[arg(short, long, default_value = "")]
    mode: String,
    /// Location where to write the file(s) to
    #[arg(short, long, default_value = "")]
    output: String,
    /// encode: Location where to find prefix files
    #[arg(short, long)]
    prefix: Option<PathBuf>,
    /// encode: Set AudioID for encoding (default: current time)
    #[arg(short = 'i', long = "id", default_value = "")]
    audio_id: String,
    /// encode: Set opus bit rate in kbps
    #[arg(short, long, default_value_t = 96)]
    bitrate: i32,
    /// encode: Use VBR encoding
    #[arg(long)]
    vbr: bool,
    /// encode: Encode Ogg Opus files again instead of copying them unchanged
    #[arg(long)]
    reencode: bool,
    /// decode: Export as single .ogg file
    #[arg(short = 's')]
    single: bool,
    /// rename: really rename files, else its a dry run
    #[arg(short = 'y')]
    really_rename: bool,
    /// rename: delete duplicates
    #[arg(short = 'd')]
    delete_duplicates: bool,
    /// info: write updated json to local file
    #[arg(short, long)]
    write: Option<PathBuf>,
    /// Set JSON file/URL with details about tonies
    #[arg(short, long)]
    json: Option<String>,
    /// Output details as: csv, json or text
    #[arg(short, long, default_value = "text")]
    format: String,
    /// increase debug message verbosity
    #[arg(short, action = ArgAction::Count)]
    verbose: u8,
    /// show licenses and disclaimer
    #[arg(long)]
    license: bool,
    files: Vec<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    if args.license {
        print!("{}", include_str!("../LICENSES.txt"));
        return ExitCode::SUCCESS;
    }

    let mut tonies = match &args.json {
        Some(source) => load_json(&[source.as_str()]),
        None => load_json(&[
            tonies::V2_URL,
            &format!("{}?source=Teddy&version={}", tonies::REVVOX_URL, env!("CARGO_PKG_VERSION")),
        ]),
    };

    let ok = match args.mode.as_str() {
        "info" => mode_info(&args, &mut tonies),
        "decode" => mode_decode(&args, &tonies),
        "encode" => mode_encode(&args),
        "rename" => mode_rename(&args, &tonies),
        _ => {
            use clap::CommandFactory;
            Args::command().print_help().ok();
            true
        }
    };

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Reads tonies.json from the first source that works
fn load_json(sources: &[&str]) -> Vec<TonieData> {
    let mut error = None;
    for source in sources {
        match read_json(source) {
            Ok(Some(t)) => return t,
            Ok(None) => return Vec::new(),
            Err(e) => error = Some(e),
        }
    }
    if let Some(e) = error {
        println!("Failed to load JSON:");
        println!("{e}");
    }
    Vec::new()
}

/// `None` if `source` is neither a URL nor a file
fn read_json(source: &str) -> Result<Option<Vec<TonieData>>, String> {
    let text = if source.starts_with("http") {
        ureq::get(source)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut r| r.body_mut().with_config().limit(64 << 20).read_to_string().map_err(|e| e.to_string()))
    } else if Path::new(source).is_file() {
        std::fs::read_to_string(source).map_err(|e| e.to_string())
    } else {
        return Ok(None);
    };
    text.and_then(|t| tonies::parse(&t).map_err(|e| e.to_string())).map(Some)
}

/// Collects the files to work on; directories are searched for Tonie files.
fn collect_tonie_files(args: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for item in args {
        let path = Path::new(item);
        if path.is_dir() {
            find_tonie_files(&mut files, path);
        } else if path.is_file() {
            files.push(path.to_path_buf());
        } else {
            return Err(format!("Error: file '{item}' does not exist"));
        }
    }
    Ok(files)
}

/// Original Tonie files are named like `500304E0`, renamed ones `<model> - <audio id> - <title>`.
fn is_tonie_file_name(name: &str) -> bool {
    if name.ends_with("0304E0") {
        return true;
    }
    let bytes = name.as_bytes();
    name.match_indices(" - ").any(|(i, _)| {
        let model_ok = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'-');
        let rest = &name[i + 3..];
        let hex_len = rest.bytes().take_while(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b)).count();
        model_ok && hex_len > 0 && rest[hex_len..].starts_with(" - ")
    })
}

fn find_tonie_files(files: &mut Vec<PathBuf>, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));

    for p in entries.iter().filter(|p| p.is_file()) {
        if is_tonie_file_name(&p.file_name().unwrap().to_string_lossy()) {
            files.push(p.clone());
        }
    }
    for p in entries.iter().filter(|p| p.is_dir()) {
        if !p.file_name().unwrap().to_string_lossy().starts_with('.') {
            find_tonie_files(files, p);
        }
    }
}

fn print_exception(context: &str, e: &Error) {
    println!();
    println!("[ERROR] Failed to process{context}");
    println!("   Message:    {e}");
}

fn mode_info(args: &Args, tonies: &mut [TonieData]) -> bool {
    let first_exists = args.files.first().map(|f| Path::new(f).exists()).unwrap_or(false);
    if !first_exists {
        println!("Error: You must specify a file");
        return false;
    }

    let format = match args.format.as_str() {
        "csv" => DumpFormat::Csv,
        "json" => DumpFormat::Json,
        _ => DumpFormat::Text,
    };
    match format {
        DumpFormat::Csv if args.files.len() > 1 || Path::new(&args.files[0]).is_dir() => println!("{CSV_HEADER}"),
        DumpFormat::Csv => {}
        DumpFormat::Json => println!("["),
        DumpFormat::Text => println!("[Mode: dump information]"),
    }

    let files = match collect_tonie_files(&args.files) {
        Ok(f) => f,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };

    let mut ok = true;
    let mut first = true;
    for file in &files {
        let mut message = String::new();
        match dump_info(&mut message, format, file, tonies, None, first) {
            Ok(()) => {
                print!("{message}");
                first = false;
            }
            Err(e) => {
                ok = false;
                report_file_error(file, &e);
            }
        }
    }

    if format == DumpFormat::Json {
        println!("]");
    }

    if let Some(path) = &args.write {
        if let Err(e) = serde_json::to_string_pretty(&tonies)
            .map_err(|e| e.to_string())
            .and_then(|j| std::fs::write(path, j).map_err(|e| e.to_string()))
        {
            println!("Failed to write JSON:");
            println!("{e}");
            ok = false;
        }
    }
    ok
}

fn report_file_error(file: &Path, e: &Error) {
    match e {
        Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => println!("File not found: {}", file.display()),
        Error::InvalidFile(_) | Error::InvalidHeader(_) => println!("File corrupt: {}", file.display()),
        _ => print_exception(&format!(" '{}'", file.display()), e),
    }
}

fn mode_decode(args: &Args, tonies: &[TonieData]) -> bool {
    let first_exists = args.files.first().map(|f| Path::new(f).exists()).unwrap_or(false);
    if !first_exists {
        println!("Error: You must specify a file");
        return false;
    }
    let files = match collect_tonie_files(&args.files) {
        Ok(f) => f,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };

    println!("[Mode: decode file]");

    let mut ok = true;
    for file in &files {
        let tonie = match TonieFile::open(file, true) {
            Ok(t) => t,
            Err(e) => {
                ok = false;
                report_file_error(file, &e);
                continue;
            }
        };

        let hash = tonie.hash_string();
        let mut tags = vec![format!("TeddyVersion={VERSION}"), format!("TeddyFile={}", file.display())];
        let info = tonies::find_for_file(tonies, &hash, tonie.header.audio_chapters.len());
        if let Some(info) = &info {
            tags.push(format!("ALBUM={}", info.title()));
            tags.push(format!("ARTIST={}", info.series.as_deref().unwrap_or("")));
            tags.push(format!("LANGUAGE={}", info.language.as_deref().unwrap_or("")));
        }
        tags.push(format!("HASH={hash}"));

        let out_dir = if args.output.is_empty() {
            file.parent().map(Path::to_path_buf).unwrap_or_default()
        } else {
            PathBuf::from(&args.output)
        };
        let out_dir = if out_dir.as_os_str().is_empty() { PathBuf::from(".") } else { out_dir };
        if !out_dir.is_dir() {
            println!("Error: Output directory '{}' does not exist", out_dir.display());
            return false;
        }

        let name = format!("{}-{:08X}", file.file_name().unwrap().to_string_lossy(), tonie.header.audio_id);
        let titles = info.as_ref().and_then(|i| i.track_names(tonie.header.audio_chapters.len()));
        if let Err(e) = tonie.dump_audio_files(&out_dir, &name, args.single, &tags, titles) {
            ok = false;
            println!("[ERROR] Failed to write .ogg/.cue'");
            println!("   Message:    {e}");
        }
    }
    ok
}

/// Prints the encoder progress like the original: ` Track   1 - Title           [....25%....]`
struct ConsoleProgress;

impl EncodeProgress for ConsoleProgress {
    fn file_start(&mut self, track: usize, path: &Path) {
        let name: String = format!("{:<15}", display_name(path)).chars().take(15).collect();
        print!(" Track {track:>3} - {name}  [");
        std::io::stdout().flush().ok();
    }

    fn progress(&mut self, fraction: f64) {
        let step = (fraction * 20.0) as i32;
        if step % 5 == 0 {
            if step != 20 {
                print!("{}%", step * 5);
            }
        } else {
            print!(".");
        }
        std::io::stdout().flush().ok();
    }

    fn file_done(&mut self) {
        println!("]");
    }

    fn failed(&mut self, message: &str) {
        println!("]");
        println!("Failed: {message}");
    }

    fn warning(&mut self, message: &str) {
        println!("[WARNING] {message}");
    }

    fn info(&mut self, message: &str) {
        println!("[INFO] {message}");
        if message.starts_with("Sorting files") {
            println!();
        }
    }
}

fn parse_audio_id(text: &str) -> Option<u32> {
    let text = text.trim();
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

fn mode_encode(args: &Args) -> bool {
    println!("[Mode: encode, {} kbps, {}]", args.bitrate, if args.vbr { "VBR" } else { "CBR" });

    if args.files.is_empty() {
        println!("Error: You must specify a directory or files to encode");
        return false;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let audio_id = if args.audio_id.is_empty() {
        (now as u32).wrapping_sub(0x5000_0000)
    } else {
        match parse_audio_id(&args.audio_id) {
            Some(id) => id,
            None => {
                println!("Error: You must specify the AudioID as hex value like 0x5E034216 or as decimal number like 1577271830");
                return false;
            }
        }
    };

    let out_location = PathBuf::from(if args.output.is_empty() { "." } else { args.output.as_str() });
    let out_file = if out_location.is_dir() {
        out_location.join("500304E0")
    } else {
        let parent = out_location.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        if !parent.is_dir() {
            println!(
                "Error: Specified output directory '{}' does not exist and file '{}' not reachable.",
                out_location.display(),
                parent.display()
            );
            return false;
        }
        out_location.clone()
    };

    let mut progress = ConsoleProgress;
    let sources: Vec<PathBuf> = args.files.iter().map(PathBuf::from).collect();
    let options = EncodeOptions {
        audio_id,
        bitrate: args.bitrate * 1000,
        vbr: args.vbr,
        prefix_dir: args.prefix.clone(),
        copy_opus: !args.reencode,
    };

    let result = (|| -> Result<(), Error> {
        let files = tonie_core::build_file_list(&sources, &mut progress)?;
        /* encode into a temporary file next to the target, so a failed run leaves nothing behind */
        let dir = out_file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        tonie_core::encode(&files, &options, temp.as_file_mut(), &mut progress)?;
        temp.persist(&out_file).map_err(|e| Error::Io(e.error))?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            println!();
            println!("Written content to {}", out_file.display());
            true
        }
        Err(e) => {
            match &e {
                Error::NotFound(m) => {
                    println!("[ERROR] Failed to process due to a missing file");
                    println!("   Message:    {m}");
                }
                Error::InvalidFile(m) => {
                    println!("[ERROR] Failed to process due to an invalid file");
                    println!("   Message:    {m}");
                }
                Error::Padding(m) | Error::Encoder(m) => {
                    println!("[ERROR] Failed to encode audio");
                    println!("   Message:    {m}");
                }
                _ => print_exception("", &e),
            }
            false
        }
    }
}

/// SHA-1 of a whole file, used to find duplicates.
fn sha1_file(path: &Path) -> std::io::Result<Vec<u8>> {
    use sha1::{Digest, Sha1};
    use std::io::Read;
    let mut hasher = Sha1::new();
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher.finalize().to_vec());
        }
        hasher.update(&buf[..n]);
    }
}

fn mode_rename(args: &Args, tonies: &[TonieData]) -> bool {
    if args.files.len() < 2 || (!Path::new(&args.files[0]).is_dir() && !Path::new(&args.files[1]).is_dir()) {
        println!("Error: You must specify a source and a destination directory");
        return false;
    }
    let dest_dir = Path::new(&args.files[1]);

    let mut files = Vec::new();
    find_tonie_files(&mut files, Path::new(&args.files[0]));

    println!(" Scan files...");
    let mut rename_list: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for file in &files {
        match TonieFile::open(file, false) {
            Ok(tonie) => {
                /* skip creative tonies for now */
                if tonie.header.audio_id == 1 {
                    println!("  '{}' -> '(creative)'", file.display());
                    continue;
                }

                let mut dest_name = "(unknown)".to_string();
                if let Some(info) = tonies::find_by_audio_id(tonies, tonie.header.audio_id) {
                    dest_name = info.title().to_string();
                    if let Some(model) = info.model.as_deref().filter(|m| !m.is_empty()) {
                        let dest = dest_dir.join(format!("{model} - {:08X} - {}", tonie.header.audio_id, remove_invalid_chars(&dest_name).trim()));
                        if *file != dest {
                            rename_list.insert(file.clone(), dest);
                        }
                    }
                }
                println!("  '{}' -> '{dest_name}'", file.display());
            }
            Err(Error::InvalidFile(_)) | Err(Error::InvalidHeader(_)) => println!("  '{}' -> (corrupt)", file.display()),
            Err(e) => report_file_error(file, &e),
        }
    }

    println!();
    println!(" Rename files...");
    let mut renamed = 0;
    for (source, dest) in &rename_list {
        println!("  Rename '{}' -> '{}'", source.display(), dest.display());
        if dest.exists() {
            match (sha1_file(dest), sha1_file(source)) {
                (Ok(a), Ok(b)) if a == b => {
                    if args.delete_duplicates {
                        if args.really_rename && std::fs::remove_file(source).is_ok() {
                            renamed += 1;
                            remove_dir_if_empty(source);
                        }
                    } else {
                        println!("    Skipped, destination file content matches");
                    }
                }
                _ => println!("    Skipped, destination file already exists but has different content"),
            }
            continue;
        }

        if args.really_rename {
            match std::fs::rename(source, dest) {
                Ok(()) => {
                    renamed += 1;
                    remove_dir_if_empty(source);
                }
                Err(_) => println!("[ERROR] Failed to rename '{}'", source.display()),
            }
        }
    }
    println!();
    println!(" Renamed {renamed} files");
    true
}

fn remove_dir_if_empty(file: &Path) {
    if let Some(dir) = file.parent() {
        if std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
            std::fs::remove_dir(dir).ok();
        }
    }
}

/// Removes characters that are not allowed in file names on any supported platform.
fn remove_invalid_chars(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tonie_file_names() {
        assert!(is_tonie_file_name("500304E0"));
        assert!(is_tonie_file_name("10000123 - 5E034216 - Some Title"));
        assert!(!is_tonie_file_name("500304E0-5E034216 - Track_01.ogg"));
        assert!(!is_tonie_file_name("info.txt"));
    }

    #[test]
    fn audio_ids() {
        assert_eq!(parse_audio_id("0x5E034216"), Some(0x5E03_4216));
        assert_eq!(parse_audio_id("1577271830"), Some(1_577_271_830));
        assert_eq!(parse_audio_id("abc"), None);
    }
}
