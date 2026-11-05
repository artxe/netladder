#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod engine;
mod process;

#[cfg(windows)]
mod elevation;
#[cfg(windows)]
mod windows;

/// Narrow enough for a laptop, wide enough that a limited row still fits
/// its checkbox, icon, name, usage bar and rate without overlapping.
const MIN_WINDOW_WIDTH: f32 = 700.0;

fn main() -> eframe::Result {
    #[cfg(windows)]
    if elevation::should_relaunch(app::preview_requested(), elevation::is_elevated())
        && elevation::relaunch_elevated()
    {
        return Ok(());
    }

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/netladder.png"))
        .expect("embedded NetLadder icon is invalid");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("NetLadder")
            .with_icon(std::sync::Arc::new(icon))
            .with_inner_size([760.0, 620.0])
            .with_min_inner_size([MIN_WINDOW_WIDTH, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "NetLadder",
        options,
        Box::new(|context| Ok(Box::new(app::NetLadderApp::new(context)))),
    )
}
