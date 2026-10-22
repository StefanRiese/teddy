//! TeddyBench: manage the audio files on a Toniebox SD card.
//!
//! The modules here contain everything except the user interface (see `main.rs`/`app.rs`).

pub mod actions;
pub mod applog;
pub mod covers;
pub mod drives;
pub mod export;
pub mod fsattr;
pub mod library;
pub mod player;
pub mod rfid;
pub mod scanner;
pub mod settings;
pub mod tagpdf;
pub mod trashcan;
pub mod uid;

/// Version shown in the title and written into exported files
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Downloads toniesV2.json from GitHub (tonies.json from api.revvox.de if that fails) if wanted, or if
/// there is no local copy, and parses it. Otherwise the local file changed last is read, so a
/// tonies.json edited by the user is used. Returns the entries and a status text for the status bar.
pub fn load_tonies_json(data_dir: &settings::DataDir, download: bool) -> (Vec<tonie_core::tonies::TonieData>, String) {
    let (v2, v1) = (data_dir.tonies_v2_json(), data_dir.tonies_json());
    let mut status = String::new();

    if download || (!v2.is_file() && !v1.is_file()) {
        let revvox = format!("{}?source=TeddyBench&version={VERSION}", tonie_core::tonies::REVVOX_URL);
        for (url, path) in [(tonie_core::tonies::V2_URL, &v2), (revvox.as_str(), &v1)] {
            let result = ureq::get(url)
                .call()
                .map_err(|e| e.to_string())
                .and_then(|mut r| r.body_mut().with_config().limit(64 << 20).read_to_string().map_err(|e| e.to_string()))
                .and_then(|json| match tonie_core::tonies::parse(&json) {
                    /* an error page must not replace a good local copy */
                    Ok(t) if !t.is_empty() => Ok((json, t)),
                    Ok(_) => Err("no entries".into()),
                    Err(e) => Err(format!("not a tonies.json ({e})")),
                });
            match result {
                Ok((json, tonies)) => {
                    applog::info(format!("Downloaded {url} ({} bytes)", json.len()));
                    if path == &v1 {
                        status = "Downloaded from api.revvox.de".into();
                    }
                    std::fs::create_dir_all(&data_dir.0).ok();
                    if std::fs::write(path, &json).is_err() {
                        status = format!("Writing Failed - is {} accessible?", path.display());
                    }
                    return (tonies, status);
                }
                Err(e) => {
                    status = format!("Download Failed: {e}");
                    applog::warning(format!("{url}: {e}"));
                }
            }
        }
    }

    let modified = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let local = match (modified(&v2), modified(&v1)) {
        (Some(a), Some(b)) if b > a => &v1,
        (Some(_), _) => &v2,
        _ => &v1,
    };
    match std::fs::read_to_string(local) {
        Ok(json) => match tonie_core::tonies::parse(&json) {
            Ok(t) => (t, status),
            Err(e) => (Vec::new(), format!("Parsing Failed: {e}")),
        },
        Err(_) if status.is_empty() => (Vec::new(), "Reading tonies.json Failed".into()),
        Err(_) => (Vec::new(), status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_local_file_changed_last() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = settings::DataDir(dir.path().to_path_buf());
        let v2 = r#"[{"article": "01-0001", "data": [{"series": "From V2", "ids": [{"audio-id": 1, "hash": "ab"}]}]}]"#;
        let v1 = r#"[{"model": "01-0001", "title": "From V1", "hash": ["AB"]}]"#;

        std::fs::write(data_dir.tonies_v2_json(), v2).unwrap();
        assert_eq!(load_tonies_json(&data_dir, false).0[0].title(), "From V2");

        /* a tonies.json edited later wins */
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(data_dir.tonies_json(), v1).unwrap();
        assert_eq!(load_tonies_json(&data_dir, false).0[0].title(), "From V1");

        std::fs::remove_file(data_dir.tonies_json()).unwrap();
        assert_eq!(load_tonies_json(&data_dir, false).0[0].title(), "From V2");
    }
}
