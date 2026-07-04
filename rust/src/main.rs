mod config;
mod executor;
mod filter;
mod gui_app;
mod gui_config;
mod reporter;
mod subscriber;
mod types;
mod wasm_host;
mod ws_client;

use anyhow::Result;
use eframe::egui;

fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Agent Loop",
        native_options,
        Box::new(|cc| Ok(Box::new(gui_app::AgentLoopApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("GUI 启动失败: {}", e))?;

    Ok(())
}
