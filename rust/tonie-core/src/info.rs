//! Text, CSV and JSON reports about a Tonie file, as printed by `teddy -m info`.

use std::fmt::Write;
use std::path::Path;

use crate::file::TonieFile;
use crate::header::HEADER_PROTO_SIZE;
use crate::tonies::TonieData;
use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DumpFormat {
    Text,
    Csv,
    Json,
}

pub const CSV_HEADER: &str = "UID;AudioID;AudioDate;HeaderLength;HeaderOK;Padding;AudioLength;AudioLengthCheck;AudioHash;AudioHashCheck;Chapters;Segments;MinSegmentsPerPage;MaxSegmentsPerPage;SegLengthSum;HighestGranule;Time;MinGranules;MaxGranules;MinTime;MaxTime;";

/// Formats a granule position (48 kHz samples) as `hh:mm:ss.ff`.
pub fn format_granule(granule: u64) -> String {
    let time = 100 * granule / 48000;
    format!(
        "{:02}:{:02}:{:02}.{:02}",
        time / 100 / 60 / 60,
        (time / 100 / 60) % 60,
        (time / 100) % 60,
        time % 100
    )
}

/// The tag UID, derived from the directory and file name (`CONTENT/0A0B0C01/500304E0` -> `E0040350010C0B0A`).
pub fn uid_from_path(path: &Path) -> String {
    let dir = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let chars: Vec<char> = format!("{dir}{name}").chars().collect();
    let is_hex = |c: char| c.is_ascii_digit() || ('A'..='F').contains(&c);

    let mut groups = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if is_hex(chars[i]) && is_hex(chars[i + 1]) {
            groups.push(format!("{}{}", chars[i], chars[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }
    groups.reverse();
    groups.concat()
}

/// UTC date as `yyyy-MM-ddTHH:mm:ss`.
pub fn format_unix_time(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    /* civil from days, see http://howardhinnant.github.io/date_algorithms.html */
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}", rem / 3600, (rem / 60) % 60, rem % 60)
}

/// Formats like .NET Framework's `float.ToString()` (7 significant digits).
pub fn format_float(v: f32) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if v == 0.0 {
        return "0".into();
    }

    let sci = format!("{:.6e}", v as f64);
    let (mantissa, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let sign = if negative { "-" } else { "" };

    if !(-4..7).contains(&exp) {
        let (first, rest) = digits.split_at(1);
        let frac = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        let exp_sign = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{first}{frac}E{exp_sign}{:02}", exp.abs());
    }

    let s = if exp >= 0 {
        let int_len = exp as usize + 1;
        let padded = format!("{digits:0<int_len$}");
        let (int, frac) = padded.split_at(int_len);
        if frac.is_empty() {
            int.to_string()
        } else {
            format!("{int}.{frac}")
        }
    } else {
        format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
    };
    format!("{sign}{s}")
}

/// `1000 * granule / 48000` in milliseconds, calculated like the original in single precision.
fn granule_ms(granule: u64) -> String {
    format_float(1000u64.wrapping_mul(granule) as f32 / 48000.0)
}

/// Milliseconds of `TimeSpan.FromSeconds(granule / 48000.0f)` (rounded to whole milliseconds).
fn timespan_ms(granule: u64) -> i64 {
    let seconds = (granule as f32 / 48000.0f32) as f64;
    (seconds * 1000.0 + 0.5) as i64
}

/// `hh:mm:ss.ff` of a duration in milliseconds.
fn format_timespan(ms: i64) -> String {
    let ms = ms.abs();
    format!(
        "{:02}:{:02}:{:02}.{:02}",
        (ms / 3_600_000) % 24,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        (ms % 1000) / 10
    )
}

/// Appends the report for one file to `out`. `first` is used for the JSON list separator.
/// When the file is found in `tonies` but its hash is missing there, the hash is added.
pub fn dump_info(out: &mut String, format: DumpFormat, path: &Path, tonies: &mut [TonieData], custom_name: Option<&str>, first: bool) -> Result<(), Error> {
    let file = TonieFile::open(path, true)?;
    let s = &file.statistics;
    let h = &file.header;

    for e in &s.errors {
        writeln!(out, "[ERROR] {e}").unwrap();
    }

    let uid = uid_from_path(path);
    let mut id = h.audio_id;
    let mut date_extra = "";
    if id < 0x5000_0000 {
        date_extra = "custom file, real date ";
        id += 0x5000_0000;
    }
    let date = format_unix_time(i64::from(id));
    let header_ok = file.header_length == HEADER_PROTO_SIZE;
    let length_ok = h.audio_length as i64 == file.audio.len() as i64;
    let hash = file.hash_string();

    match format {
        DumpFormat::Csv => {
            let chapters: String = h.audio_chapters.iter().map(|c| format!("{c} ")).collect();
            writeln!(
                out,
                "{uid};{:08X};{date};{};{};{};{};{};{hash};{};{chapters};{};{};{};{};{};{};{};{};{};{};",
                h.audio_id,
                file.header_length,
                if header_ok { "[OK]" } else { "[WARNING: EXTRA DATA]" },
                h.padding.len(),
                h.audio_length,
                if length_ok { "[OK]" } else { "[INCORRECT]" },
                if file.hash_correct { "[OK]" } else { "[INCORRECT]" },
                s.total_segments,
                s.min_segments,
                s.max_segments,
                s.segment_length,
                s.highest_granule,
                format_granule(s.highest_granule),
                s.min_granule,
                s.max_granule,
                granule_ms(s.min_granule),
                granule_ms(s.max_granule),
            )
            .unwrap();
        }

        DumpFormat::Text => {
            writeln!(out, "Dump of {} (UID {uid}):", path.display()).unwrap();
            writeln!(out, "  Header: AudioID     0x{:08X} ({date_extra}{date})", h.audio_id).unwrap();

            let mut titles: &[String] = &[];
            let mut json_hash: Option<String> = None;
            let found = tonies.iter_mut().find(|t| t.hash.contains(&hash));
            if let Some(info) = found {
                let index = info.audio_ids().iter().position(|&a| a == i64::from(h.audio_id));
                json_hash = index.and_then(|i| info.hash.get(i)).filter(|x| !x.is_empty()).cloned();
                writeln!(out, "  Header: JSON Name   '{}'", info.title()).unwrap();

                /* remember the hash for this audio ID, e.g. to write an updated tonies.json */
                if let (Some(i), None) = (index, &json_hash) {
                    if info.hash.len() <= i {
                        info.hash.resize(i + 1, String::new());
                    }
                    info.hash[i] = hash.clone();
                }
                titles = &info.tracks;
            }
            if let Some(name) = custom_name.filter(|n| !n.is_empty()) {
                writeln!(out, "  Header: Custom Name '{name}'").unwrap();
            }

            let json_state = match &json_hash {
                Some(j) if *j == hash => "[JSON MATCH]",
                Some(_) => "[JSON MISMATCH]",
                None => "[NO JSON INFO]",
            };
            writeln!(
                out,
                "  Header: Length      0x{:08X} {}",
                file.header_length,
                if header_ok { "[OK]" } else { " [WARNING: EXTRA DATA]" }
            )
            .unwrap();
            writeln!(out, "  Header: Padding     0x{:08X}", h.padding.len()).unwrap();
            writeln!(
                out,
                "  Header: AudioLen    0x{:08X} {}",
                h.audio_length,
                if length_ok { "[OK]" } else { "[INCORRECT]" }
            )
            .unwrap();
            writeln!(
                out,
                "  Header: Checksum    {hash} {} {json_state}",
                if file.hash_correct { "[OK]" } else { "[INCORRECT]" }
            )
            .unwrap();
            writeln!(out, "  Header: Chapters    ").unwrap();

            let mut prev_ms = 0i64;
            let chapters = &h.audio_chapters;
            for track in 1..=chapters.len() {
                let page = if track < chapters.len() { chapters[track] } else { file.highest_page() };
                let length = match file.granule_by_page(page) {
                    Some(granule) => {
                        let ms = timespan_ms(granule);
                        let s = format_timespan(ms - prev_ms);
                        prev_ms = ms;
                        s
                    }
                    None => format!("@{page}"),
                };
                let title = titles.get(track - 1).map(String::as_str).unwrap_or("");
                writeln!(out, "    Track #{track:02}  {length}  {title}").unwrap();
            }

            writeln!(
                out,
                "  Ogg: Segments       {} (min: {} max: {} per OggPage)",
                s.total_segments, s.min_segments, s.max_segments
            )
            .unwrap();
            writeln!(out, "  Ogg: net payload    {} byte", s.segment_length).unwrap();
            writeln!(
                out,
                "  Ogg: granules       total: {} ({} hh:mm:ss.ff)",
                s.highest_granule,
                format_granule(s.highest_granule)
            )
            .unwrap();
            writeln!(
                out,
                "  Ogg: granules/page  min: {} max: {} ({}ms - {}ms)",
                s.min_granule,
                s.max_granule,
                granule_ms(s.min_granule),
                granule_ms(s.max_granule)
            )
            .unwrap();
            writeln!(out).unwrap();
        }

        DumpFormat::Json => {
            if !first {
                writeln!(out, ",").unwrap();
            }
            let chapters: String = h.audio_chapters.iter().map(|c| format!(", {c}")).collect();
            let tf = |b: bool| if b { "TRUE" } else { "FALSE" };
            writeln!(out, "  {{").unwrap();
            writeln!(out, "    \"uid\": \"{uid}\",").unwrap();
            writeln!(out, "    \"audio_id\": \"{:08X}\",", h.audio_id).unwrap();
            writeln!(out, "    \"audio_date\": \"{date}\",").unwrap();
            writeln!(out, "    \"header_length\": {},", file.header_length).unwrap();
            writeln!(out, "    \"header_ok\": \"{}\",", tf(header_ok)).unwrap();
            writeln!(out, "    \"padding\": {},", h.padding.len()).unwrap();
            writeln!(out, "    \"audio_length\": {},", h.audio_length).unwrap();
            writeln!(out, "    \"audio_length_check\": \"{}\",", tf(length_ok)).unwrap();
            writeln!(out, "    \"audio_hash\": \"{hash}\",").unwrap();
            writeln!(out, "    \"audio_hash_check\": \"{}\",", tf(file.hash_correct)).unwrap();
            writeln!(out, "    \"chapters\": [ 0{chapters} ],").unwrap();
            writeln!(out, "    \"segments\": {},", s.total_segments).unwrap();
            writeln!(out, "    \"min_segments_per_page\": {},", s.min_segments).unwrap();
            writeln!(out, "    \"max_segments_per_page\": {},", s.max_segments).unwrap();
            writeln!(out, "    \"segment_length_sum\": {},", s.segment_length).unwrap();
            writeln!(out, "    \"highest_granule\": {},", s.highest_granule).unwrap();
            writeln!(out, "    \"time\": \"{}\",", format_granule(s.highest_granule)).unwrap();
            writeln!(out, "    \"min_granules\": {},", s.min_granule).unwrap();
            writeln!(out, "    \"max_granules\": {},", s.max_granule).unwrap();
            writeln!(out, "    \"min_time\": {},", granule_ms(s.min_granule)).unwrap();
            writeln!(out, "    \"max_time\": {}", granule_ms(s.max_granule)).unwrap();
            writeln!(out, "  }}").unwrap();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_formatting_matches_dotnet() {
        assert_eq!(format_float(340.0), "340");
        assert_eq!(format_float(20.0), "20");
        assert_eq!(format_float(0.5), "0.5");
        assert_eq!(format_float(1.0 / 3.0), "0.3333333");
        assert_eq!(format_float(123456.7), "123456.7");
        assert_eq!(format_float(1.921_535_9e17), "1.921536E+17");
        assert_eq!(format_float(0.00001), "1E-05");
        assert_eq!(format_float(-2.5), "-2.5");
    }

    #[test]
    fn dates_and_uids() {
        assert_eq!(format_unix_time(0x5E03_4216), "2019-12-25T11:03:50");
        assert_eq!(format_unix_time(0), "1970-01-01T00:00:00");
        assert_eq!(uid_from_path(Path::new("CONTENT/0A0B0C01/500304E0")), "E0040350010C0B0A");
        assert_eq!(format_granule(635_520), "00:00:13.24");
    }
}
