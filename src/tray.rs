//! StatusNotifierItem: состояние языка, меню и именованные Material PNG для хостов MATE/Ayatana.

use punto_rs::{layout::Layout, service::Shared};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub struct Tray {
    pub shared: Shared,
    pub show: Arc<AtomicBool>,
    pub quit: Arc<AtomicBool>,
    pub icon_directory: tempfile::TempDir,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "punto-rs".into()
    }
    fn title(&self) -> String {
        let state = self.shared.lock().unwrap();
        format!(
            "Punto RS · {} · {}",
            state.engine.layout.label(),
            if !state.connected {
                "Ввод не подключён"
            } else if !state.focused {
                "Нет доступного поля"
            } else if state.automatic() {
                "Авто"
            } else if state.config.automatic {
                "Исключение или пауза"
            } else {
                "Вручную"
            }
        )
    }
    // MATE/Ayatana resolves a named icon; IconPixmap alone can show a generic fallback.
    fn icon_theme_path(&self) -> String {
        self.icon_directory.path().to_string_lossy().into_owned()
    }
    fn icon_name(&self) -> String {
        let state = self.shared.lock().unwrap();
        icon_asset(state.engine.layout, state.automatic()).0.into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let state = self.shared.lock().unwrap();
        vec![icon(state.engine.layout, state.automatic())]
    }
    fn activate(&mut self, _: i32, _: i32) {
        self.show.store(true, Ordering::Relaxed);
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let title = self.title();
        let state = self.shared.lock().unwrap();
        ksni::ToolTip {
            title,
            description: format!("{}\n{}", state.engine.app, state.message),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let state = self.shared.lock().unwrap();
        let automatic = state.config.automatic;
        let layout = state.engine.layout.label();
        vec![
            CheckmarkItem {
                label: "Автопереключение".into(),
                checked: automatic,
                activate: Box::new(|tray: &mut Self| {
                    let mut state = tray.shared.lock().unwrap();
                    state.config.automatic = !state.config.automatic;
                    if let Err(e) = state.config.save(&state.path) {
                        state.message = e.to_string();
                    }
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: format!("Раскладка: {layout} — переключить"),
                activate: Box::new(|tray: &mut Self| {
                    let mut state = tray.shared.lock().unwrap();
                    state.engine.layout = state.engine.layout.other();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Настройки…".into(),
                icon_name: "preferences-system".into(),
                activate: Box::new(|tray: &mut Self| tray.show.store(true, Ordering::Relaxed)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Выход".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| tray.quit.store(true, Ordering::Relaxed)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

const ICONS: [(&str, &[u8]); 4] = [
    (
        "punto-rs-en",
        include_bytes!("../assets/material-icons/punto-rs-en.png"),
    ),
    (
        "punto-rs-ru",
        include_bytes!("../assets/material-icons/punto-rs-ru.png"),
    ),
    (
        "punto-rs-en-paused",
        include_bytes!("../assets/material-icons/punto-rs-en-paused.png"),
    ),
    (
        "punto-rs-ru-paused",
        include_bytes!("../assets/material-icons/punto-rs-ru-paused.png"),
    ),
];

pub fn icon_directory() -> anyhow::Result<tempfile::TempDir> {
    // MATE/Ayatana requires named files; it ignores IconPixmap.
    let directory = tempfile::Builder::new()
        .prefix("punto-rs-icons-")
        .tempdir()?;
    for (name, png) in ICONS {
        std::fs::write(directory.path().join(format!("{name}.png")), png)?;
    }
    Ok(directory)
}

fn icon_asset(layout: Layout, active: bool) -> (&'static str, &'static [u8]) {
    ICONS[match (layout, active) {
        (Layout::En, true) => 0,
        (Layout::Ru, true) => 1,
        (Layout::En, false) => 2,
        (Layout::Ru, false) => 3,
    }]
}

fn icon(layout: Layout, active: bool) -> ksni::Icon {
    let image = eframe::icon_data::from_png_bytes(icon_asset(layout, active).1)
        .expect("checked-in Material tray icon");
    let mut data = image.rgba;
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.rotate_right(1);
    }
    ksni::Icon {
        width: image.width as i32,
        height: image.height as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_icons_match_pixmaps_for_every_state() {
        let directory = icon_directory().unwrap();
        let mut images = Vec::new();
        for layout in [Layout::En, Layout::Ru] {
            for active in [false, true] {
                let (name, png) = icon_asset(layout, active);
                assert_eq!(
                    std::fs::read(directory.path().join(format!("{name}.png"))).unwrap(),
                    png
                );
                let image = eframe::icon_data::from_png_bytes(png).unwrap();
                let pixmap = icon(layout, active);
                assert_eq!((pixmap.width, pixmap.height), (64, 64));
                for (rgba, argb) in image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(pixmap.data.as_chunks::<4>().0)
                {
                    assert_eq!(*argb, [rgba[3], rgba[0], rgba[1], rgba[2]]);
                }
                assert!(!images.contains(&pixmap.data));
                images.push(pixmap.data);
            }
        }
    }
}
