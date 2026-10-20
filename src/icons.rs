//! Встроенные Material-иконки: установка приложения не зависит от каталога исходной библиотеки.

use eframe::egui;

pub fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/material-icons/app.png"))
        .expect("checked-in application icon")
}

pub struct Icons {
    pub app: egui::TextureHandle,
    pub keyboard: egui::TextureHandle,
    pub settings: egui::TextureHandle,
    pub translate: egui::TextureHandle,
    pub undo: egui::TextureHandle,
    pub swap: egui::TextureHandle,
}

impl Icons {
    pub fn load(ctx: &egui::Context) -> Self {
        let load = |name: &str, png: &[u8]| {
            let icon = eframe::icon_data::from_png_bytes(png).expect("checked-in Material icon");
            ctx.load_texture(
                name,
                egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                ),
                Default::default(),
            )
        };
        Self {
            app: load("app", include_bytes!("../assets/material-icons/app.png")),
            keyboard: load(
                "keyboard",
                include_bytes!("../assets/material-icons/keyboard.png"),
            ),
            settings: load(
                "settings",
                include_bytes!("../assets/material-icons/settings.png"),
            ),
            translate: load(
                "translate",
                include_bytes!("../assets/material-icons/translate.png"),
            ),
            undo: load("undo", include_bytes!("../assets/material-icons/undo.png")),
            swap: load(
                "swap",
                include_bytes!("../assets/material-icons/swap_horiz.png"),
            ),
        }
    }
}
