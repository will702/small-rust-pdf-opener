mod app;
mod error;
mod export;
mod ocr;
mod page_range;
mod pdf;
mod sign;
mod theme;

use std::path::PathBuf;
use std::sync::Arc;

use app::PdfApp;

fn load_app_icon() -> Arc<egui::IconData> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon-256.png"))
        .expect("app icon");
    Arc::new(icon)
}

fn main() -> eframe::Result<()> {
    let initial = std::env::args().nth(1).map(PathBuf::from);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 800.0])
            .with_title("PDF Opener")
            // Without this, eframe injects the default egui "e" icon via
            // NSApplication.setApplicationIconImage, which replaces the Dock
            // icon from AppIcon.icns as soon as the window opens.
            .with_icon(load_app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "PDF Opener",
        native_options,
        Box::new(move |cc| Ok(Box::new(PdfApp::new(cc, initial)))),
    )
}
