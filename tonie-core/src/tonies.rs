//! The tonies.json database with titles, track names and covers.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TonieData {
    #[serde(rename = "no", default)]
    pub sort_number: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(rename = "audio_id", default)]
    pub audio_id_strings: Vec<String>,
    #[serde(default)]
    pub hash: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub series: Option<String>,
    #[serde(default)]
    pub episodes: Option<String>,
    #[serde(default)]
    pub tracks: Vec<String>,
    #[serde(default)]
    pub release: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub pic: Option<String>,
    /// Fields not known to this version, kept when writing the file again
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl TonieData {
    /// Audio IDs as numbers; empty and "na" entries are skipped.
    pub fn audio_ids(&self) -> Vec<i64> {
        self.audio_id_strings
            .iter()
            .filter(|s| !s.is_empty() && s.as_str() != "na")
            .filter_map(|s| s.parse().ok())
            .collect()
    }

    /// Key for sorting by language and number.
    pub fn sort_string(&self) -> String {
        let mut ret = self.language.clone().unwrap_or_default();
        if let Some(n) = self.sort_number.as_deref().filter(|n| !n.is_empty() && *n != "na") {
            if let Ok(n) = n.parse::<i64>() {
                ret += &format!("{n:04}");
            }
        }
        ret
    }

    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or("")
    }

    /// The track names, if they fit a file with `tracks` tracks. A re-release can have another
    /// number of tracks than the version the names were written for; names assigned by position
    /// would then be wrong.
    pub fn track_names(&self, tracks: usize) -> Option<&[String]> {
        (!self.tracks.is_empty() && self.tracks.len() == tracks).then_some(self.tracks.as_slice())
    }
}

/// toniesV2.json from the tonies-json repository, the main source
pub const V2_URL: &str = "https://raw.githubusercontent.com/toniebox-reverse-engineering/tonies-json/release/toniesV2.json";
/// The original tonies.json, used when GitHub is not available
pub const REVVOX_URL: &str = "https://api.revvox.de/tonies.json";

/// An entry of toniesV2.json: an article (model number) with its variants
#[derive(Deserialize)]
struct ArticleV2 {
    article: String,
    #[serde(default)]
    data: Vec<DataV2>,
}

#[derive(Deserialize)]
struct DataV2 {
    #[serde(default)]
    series: Option<String>,
    #[serde(default)]
    episode: Option<String>,
    #[serde(default)]
    release: Option<serde_json::Value>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(rename = "track-desc", default)]
    tracks: Vec<String>,
    #[serde(default)]
    ids: Vec<IdV2>,
}

#[derive(Deserialize)]
struct IdV2 {
    #[serde(rename = "audio-id", default)]
    audio_id: Option<serde_json::Value>,
    #[serde(default)]
    hash: Option<String>,
}

/// A number or string as text, the way tonies.json has it
fn json_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl From<(&str, DataV2)> for TonieData {
    fn from((article, d): (&str, DataV2)) -> TonieData {
        let series = d.series.unwrap_or_default();
        let title = match d.episode.as_deref().filter(|e| !e.is_empty()) {
            Some(episode) => format!("{series} - {episode}"),
            None => series.clone(),
        };
        TonieData {
            model: Some(article.to_string()),
            audio_id_strings: d.ids.iter().filter_map(|i| i.audio_id.as_ref().and_then(json_text)).collect(),
            hash: d.ids.iter().filter_map(|i| i.hash.as_ref().map(|h| h.to_ascii_uppercase())).collect(),
            title: Some(title),
            series: Some(series),
            episodes: d.episode,
            tracks: d.tracks,
            release: d.release.as_ref().and_then(json_text),
            language: d.language,
            category: d.category,
            pic: d.image,
            ..Default::default()
        }
    }
}

/// Reads tonies.json, or toniesV2.json (one entry per article and variant).
pub fn parse(json: &str) -> Result<Vec<TonieData>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let is_v2 = value.as_array().and_then(|a| a.first()).is_some_and(|e| e.get("article").is_some());
    if !is_v2 {
        return serde_json::from_value(value);
    }
    let articles: Vec<ArticleV2> = serde_json::from_value(value)?;
    Ok(articles
        .into_iter()
        .flat_map(|a| {
            let article = a.article;
            a.data.into_iter().map(move |d| TonieData::from((article.as_str(), d)))
        })
        .collect())
}

pub fn find_by_hash<'a>(infos: &'a [TonieData], hash: &str) -> Option<&'a TonieData> {
    infos.iter().find(|t| t.hash.iter().any(|h| h == hash))
}

/// The entry for a Tonie file with `hash` and `tracks` tracks. Its track names may not fit the file,
/// e.g. for a re-release with other tracks; then names that fit are taken from another entry: one
/// that also lists the hash, or one of the same series, episode and language (the same Tonie under
/// another model number). Without fitting names, the entry is returned as it is.
pub fn find_for_file(infos: &[TonieData], hash: &str, tracks: usize) -> Option<TonieData> {
    let with_hash: Vec<&TonieData> = infos.iter().filter(|t| t.hash.iter().any(|h| h == hash)).collect();
    let first = *with_hash.first()?;
    if let Some(fits) = with_hash.iter().find(|t| t.track_names(tracks).is_some()) {
        return Some((*fits).clone());
    }
    let same = |t: &&TonieData| {
        let key = |t: &TonieData| {
            (
                t.series.clone().unwrap_or_default().to_lowercase(),
                t.episodes.clone().unwrap_or_default().to_lowercase(),
                t.language.clone(),
            )
        };
        key(t) == key(first) && first.episodes.as_deref().is_some_and(|e| !e.is_empty())
    };
    let mut info = first.clone();
    if let Some(names) = infos.iter().filter(same).find_map(|t| t.track_names(tracks)) {
        info.tracks = names.to_vec();
    }
    Some(info)
}

pub fn find_by_audio_id(infos: &[TonieData], audio_id: u32) -> Option<&TonieData> {
    infos.iter().find(|t| t.audio_ids().contains(&i64::from(audio_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tonies_v2() {
        let json = r#"[{"article": "01-0019", "data": [{
            "series": "Der Grüffelo", "episode": "Der Grüffelo", "release": 1481155200, "language": "de-de",
            "category": "audio-play", "runtime": 21, "image": "https://example.com/01-0019.png",
            "track-desc": ["Der Grüffelo", "Instrumental"],
            "ids": [{"audio-id": 1726470185, "hash": "9c396b736790082a51285ac73c55fdad9b62029f", "size": 15512200, "tracks": 5, "confidence": 2},
                    {"audio-id": 1483546545, "hash": "cd1f1f3cf94f6aa6017c97107a929f74e662be57", "tracks": 5}]
        }]}, {"article": "10000123", "data": [{"series": "Peter Rabbit", "episode": null, "ids": []}]}]"#;
        let tonies = parse(json).unwrap();
        assert_eq!(tonies.len(), 2);
        let t = &tonies[0];
        assert_eq!(t.model.as_deref(), Some("01-0019"));
        assert_eq!(t.title(), "Der Grüffelo - Der Grüffelo");
        assert_eq!(t.tracks, ["Der Grüffelo", "Instrumental"]);
        assert_eq!(t.pic.as_deref(), Some("https://example.com/01-0019.png"));
        assert_eq!(t.release.as_deref(), Some("1481155200"));
        assert_eq!(t.audio_ids(), [1726470185, 1483546545]);
        /* hashes are compared in upper case, like in tonies.json */
        assert!(find_by_hash(&tonies, "CD1F1F3CF94F6AA6017C97107A929F74E662BE57").is_some());
        assert_eq!(find_by_audio_id(&tonies, 1726470185).map(|t| t.title()), Some("Der Grüffelo - Der Grüffelo"));
        /* no episode: just the series, not "Peter Rabbit - None" */
        assert_eq!(tonies[1].title(), "Peter Rabbit");

        /* the names only for a version with as many tracks */
        assert_eq!(t.track_names(2).map(<[String]>::len), Some(2));
        assert_eq!(t.track_names(5), None);
        assert_eq!(tonies[1].track_names(0), None);
    }

    #[test]
    fn finds_track_names_that_fit_the_file() {
        let entry = |model: &str, episode: &str, hash: &str, names: usize| TonieData {
            model: Some(model.into()),
            series: Some("Series".into()),
            episodes: Some(episode.into()),
            language: Some("de-de".into()),
            hash: vec![hash.into()],
            tracks: (1..=names).map(|i| format!("{model} {i}")).collect(),
            ..Default::default()
        };
        let infos = vec![
            entry("01-0001", "Episode", "AA", 8),
            /* a re-release under another model number, with other tracks */
            entry("10000001", "episode", "BB", 4),
            /* the same audio in two products */
            entry("01-0002", "Other", "CC", 3),
            entry("10000002", "Other", "CC", 5),
        ];
        assert_eq!(find_for_file(&infos, "AA", 8).unwrap().tracks[0], "01-0001 1");
        /* names of the same series and episode, the rest of the entry stays */
        let t = find_for_file(&infos, "AA", 4).unwrap();
        assert_eq!((t.model.as_deref(), t.tracks.len(), t.tracks[0].as_str()), (Some("01-0001"), 4, "10000001 1"));
        /* the entry with the same hash whose names fit */
        assert_eq!(find_for_file(&infos, "CC", 5).unwrap().model.as_deref(), Some("10000002"));
        /* nothing fits: the entry as it is */
        assert_eq!(find_for_file(&infos, "CC", 7).unwrap().model.as_deref(), Some("01-0002"));
        assert!(find_for_file(&infos, "DD", 1).is_none());
    }

    #[test]
    fn reads_tonies_v1() {
        let json = r#"[{"no": "0", "model": "01-0143", "audio_id": ["1539358350"], "hash": ["5A8E64A9D90F1AEF66F3B2505218DCE5BBBD59B5"],
            "title": "Heule Eule - Geschichten", "series": "Heule Eule", "tracks": ["Heule Eule"], "pic": "https://example.com/a.png"}]"#;
        let tonies = parse(json).unwrap();
        assert_eq!(tonies[0].title(), "Heule Eule - Geschichten");
        assert_eq!(tonies[0].sort_number.as_deref(), Some("0"));
        assert!(find_by_hash(&tonies, "5A8E64A9D90F1AEF66F3B2505218DCE5BBBD59B5").is_some());
        assert!(parse("[]").unwrap().is_empty());
        assert!(parse("<html>403 Forbidden</html>").is_err());
    }
}
