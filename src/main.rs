#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod engine;
mod process;

#[cfg(windows)]
mod windows;

fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/netladder.png"))
        .expect("embedded NetLadder icon is invalid");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("NetLadder")
            .with_icon(std::sync::Arc::new(icon))
            .with_inner_size([760.0, 620.0])
            .with_min_inner_size([620.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "NetLadder",
        options,
        Box::new(|context| Ok(Box::new(app::NetLadderApp::new(context)))),
    )
}
