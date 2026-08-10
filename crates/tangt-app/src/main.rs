#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod camera;
mod canvas;
mod editor;
mod export;
mod files;
mod help;
mod llm;
mod mcp;
mod theme;

const USAGE: &str = "\
tangt — Gantt charts as code

  tangt [FILE.tangt]            open the app (optionally on a chart)
  tangt mcp-serve FILE.tangt    serve that chart to an MCP client over stdio
  tangt --help                  this message
";

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // A second personality, dispatched before any window opens.
        Some("mcp-serve") => match args.next() {
            Some(path) => mcp::headless::main(std::path::PathBuf::from(path)),
            None => {
                eprintln!("tangt mcp-serve needs a chart file\n\n{USAGE}");
                std::process::exit(2);
            }
        },
        Some("--help" | "-h") => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        Some("--version" | "-V") => {
            println!("tangt {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        _ => {}
    }

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
