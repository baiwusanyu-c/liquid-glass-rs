#![windows_subsystem = "windows"]

#[path = "frosted_liquid/app.rs"]
mod app;

fn main() -> windows::core::Result<()> {
    app::run(app::Preset::FrostedLiquid)
}
