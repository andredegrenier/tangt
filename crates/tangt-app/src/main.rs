#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod camera;
mod canvas;
mod editor;
mod export;
mod files;
mod help;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("tangt"),
        ..Default::default()
    };
    eframe::run_native(
        "tangt",
        options,
        Box::new(|cc| Ok(Box::new(app::TangtApp::new(cc)))),
    )
}
