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
}

pub fn parse(json: &str) -> Result<Vec<TonieData>, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn find_by_hash<'a>(infos: &'a [TonieData], hash: &str) -> Option<&'a TonieData> {
    infos.iter().find(|t| t.hash.iter().any(|h| h == hash))
}

pub fn find_by_audio_id(infos: &[TonieData], audio_id: u32) -> Option<&TonieData> {
    infos.iter().find(|t| t.audio_ids().contains(&i64::from(audio_id)))
}
