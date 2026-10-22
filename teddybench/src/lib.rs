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

/// Downloads tonies.json if wanted (or if there is no local copy) and parses it.
/// Returns the entries and a status text for the status bar.
pub fn load_tonies_json(data_dir: &settings::DataDir, download: bool) -> (Vec<tonie_core::tonies::TonieData>, String) {
    let path = data_dir.tonies_json();
    let mut status = String::new();

    if download || !path.is_file() {
        let primary = format!("https://api.revvox.de/tonies.json?source=TeddyBench&version={VERSION}");
        for url in [primary.as_str(), tonie_core::tonies::FALLBACK_URL] {
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
                    applog::info(format!("Downloaded tonies.json from {url} ({} bytes)", json.len()));
                    if url == tonie_core::tonies::FALLBACK_URL {
                        status = "Downloaded from GitHub".into();
                    }
                    std::fs::create_dir_all(&data_dir.0).ok();
                    if std::fs::write(&path, &json).is_err() {
                        status = "Writing Failed - is the tonies.json accessible?".into();
                    }
                    return (tonies, status);
                }
                Err(e) => {
                    status = format!("Download Failed: {e}");
                    applog::warning(format!("tonies.json from {url}: {e}"));
                }
            }
        }
    }

    match std::fs::read_to_string(&path) {
        Ok(json) => match tonie_core::tonies::parse(&json) {
            Ok(t) => (t, status),
            Err(e) => (Vec::new(), format!("Parsing Failed: {e}")),
        },
        Err(_) if status.is_empty() => (Vec::new(), "Reading tonies.json Failed".into()),
        Err(_) => (Vec::new(), status),
    }
}
