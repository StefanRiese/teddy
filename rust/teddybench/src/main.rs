#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result {
    let icon = image::load_from_memory(include_bytes!("../assets/custom.png")).ok().map(|img| {
        let img = img.resize_exact(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8();
        eframe::egui::IconData {
            width: img.width(),
            height: img.height(),
            rgba: img.into_raw(),
        }
    });

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([640.0, 400.0])
        .with_drag_and_drop(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    eframe::run_native(
        "TeddyBench",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(app::TeddyBenchApp::new(cc)))),
    )
}
