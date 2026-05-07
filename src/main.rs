// Hide the console window on Windows in release builds
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod crypto;
mod model;
mod portable;
mod storage;

use app::App;
use eframe::egui;
use egui::{FontFamily, FontId, TextStyle};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([820.0, 520.0])
            .with_title("KeyVault"),
        ..Default::default()
    };

    eframe::run_native(
        "KeyVault",
        options,
        Box::new(|cc| {
            let mut style = (*cc.egui_ctx.style()).clone();

            // Larger, more readable type scale
            style.text_styles = [
                (TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(16.0, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(16.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(15.0, FontFamily::Monospace)),
                (TextStyle::Small, FontId::new(13.0, FontFamily::Proportional)),
            ]
            .into();

            // Roomier spacing & buttons
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 6.0);
            style.spacing.interact_size.y = 30.0;
            style.spacing.icon_width = 18.0;

            cc.egui_ctx.set_style(style);
            Ok(Box::new(App::new(cc)))
        }),
    )
}
