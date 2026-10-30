#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod preferences;
mod preview_geometry;
mod preview_image;
mod progress;
mod view;

use std::path::PathBuf;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|value| value == "--version" || value == "-V")
    {
        println!("AssetStudio {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if arguments
        .first()
        .is_some_and(|value| value == "--help" || value == "-h")
    {
        println!(
            "AssetStudio {}\n\nUsage: AssetStudio [FILE_OR_FOLDER]\n\n  --help       Show this help\n  --version    Show version\n  --smoke-test  Check the desktop interface without opening a window",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if arguments
        .first()
        .is_some_and(|value| value == "--smoke-test")
    {
        return app::smoke_test();
    }
    if arguments.len() > 1
        || arguments
            .first()
            .is_some_and(|value| value.to_string_lossy().starts_with('-'))
    {
        return Err("expected one file or folder; use --help for usage".to_owned());
    }
    let initial_path = arguments.first().map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("AssetStudio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1024.0, 640.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "AssetStudio",
        options,
        Box::new(move |context| {
            Ok(Box::new(app::StudioApp::new(
                &context.egui_ctx,
                initial_path,
            )))
        }),
    )
    .map_err(|error| error.to_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("AssetStudio: {error}");
            ExitCode::FAILURE
        }
    }
}
