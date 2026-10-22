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
pub mod uid;

/// Version shown in the title and written into exported files
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Downloads tonies.json if wanted (or if there is no local copy) and parses it.
/// Returns the entries and a status text for the status bar.
pub fn load_tonies_json(data_dir: &settings::DataDir, download: bool) -> (Vec<tonie_core::tonies::TonieData>, String) {
    let path = data_dir.tonies_json();
    let mut status = String::new();

    if download || !path.is_file() {
        let url = format!("https://api.revvox.de/tonies.json?source=TeddyBench&version={VERSION}");
        match ureq::get(&url)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut r| r.body_mut().with_config().limit(64 << 20).read_to_string().map_err(|e| e.to_string()))
        {
            Ok(json) if !json.is_empty() => {
                applog::info(format!("Downloaded tonies.json ({} bytes)", json.len()));
                std::fs::create_dir_all(&data_dir.0).ok();
                if std::fs::write(&path, &json).is_err() {
                    status = "Writing Failed - is the tonies.json accessible?".into();
                }
                return match tonie_core::tonies::parse(&json) {
                    Ok(t) => (t, status),
                    Err(e) => (Vec::new(), format!("Parsing Failed: {e}")),
                };
            }
            Ok(_) => status = "Download Failed: empty response".into(),
            Err(e) => status = format!("Download Failed: {e}"),
        }
        applog::warning(format!("tonies.json: {status}"));
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
