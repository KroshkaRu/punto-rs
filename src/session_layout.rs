//! Адаптеры уже настроенных источников Fcitx5, GNOME и Plasma для сеансов Wayland.

use crate::layout::Layout;
use anyhow::{Context, Result, ensure};
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};

#[derive(Clone, Copy)]
enum Driver {
    Fcitx,
    Gnome,
    Kde,
}
pub struct SessionLayout {
    connection: Connection,
    driver: Driver,
    pub reported: Option<Layout>,
}
impl SessionLayout {
    pub fn discover() -> Result<Option<Self>> {
        let connection = zbus::blocking::connection::Builder::session()?
            .method_timeout(Duration::from_millis(500))
            .build()?;
        let bus = zbus::blocking::fdo::DBusProxy::new(&connection)?;
        let mut driver = None;
        for (name, candidate) in [
            ("org.fcitx.Fcitx5", Driver::Fcitx),
            ("org.punto.Switcher.Focus", Driver::Gnome),
            ("org.kde.keyboard", Driver::Kde),
        ] {
            if bus.name_has_owner(name.try_into()?)? {
                driver = Some(candidate);
                break;
            }
        }
        drop(bus);
        Ok(driver.map(|driver| Self {
            connection,
            driver,
            reported: None,
        }))
    }
    fn proxy(&self) -> Result<Proxy<'_>> {
        let (name, path, interface) = match self.driver {
            Driver::Fcitx => (
                "org.fcitx.Fcitx5",
                "/controller",
                "org.fcitx.Fcitx.Controller1",
            ),
            Driver::Gnome => (
                "org.punto.Switcher.Focus",
                "/org/punto/Switcher/Focus",
                "org.punto.Switcher.Focus",
            ),
            Driver::Kde => ("org.kde.keyboard", "/Layouts", "org.kde.KeyboardLayouts"),
        };
        Ok(Proxy::new(&self.connection, name, path, interface)?)
    }
    pub fn current(&self) -> Result<Option<Layout>> {
        let proxy = self.proxy()?;
        Ok(match self.driver {
            Driver::Fcitx => language(&proxy.call::<_, _, String>("CurrentInputMethod", &())?),
            Driver::Gnome => language(&proxy.call::<_, _, String>("GetLayout", &())?),
            Driver::Kde => {
                let index: u32 = proxy.call("getLayout", &())?;
                let layouts: Vec<(String, String, String)> = proxy.call("getLayoutsList", &())?;
                layouts.get(index as usize).and_then(|row| language(&row.0))
            }
        })
    }
    pub fn switch(&self, layout: Layout) -> Result<()> {
        let proxy = self.proxy()?;
        match self.driver {
            Driver::Fcitx => {
                let current: String = proxy.call("CurrentInputMethod", &())?;
                ensure!(
                    language(&current).is_some(),
                    "Активен другой движок Fcitx5; переключение отклонено"
                );
                let group: String = proxy.call("CurrentInputMethodGroup", &())?;
                let (_, entries): (String, Vec<(String, String)>) =
                    proxy.call("InputMethodGroupInfo", &(group,))?;
                let entry = entries
                    .iter()
                    .find(|(name, _)| language(name) == Some(layout))
                    .context("RU/EN отсутствует в текущей группе Fcitx5")?;
                let _: () = proxy.call("SetCurrentIM", &(&entry.0,))?;
            }
            Driver::Gnome => {
                let changed: bool = proxy.call(
                    "SetLayout",
                    &(match layout {
                        Layout::En => "us",
                        Layout::Ru => "ru",
                    },),
                )?;
                ensure!(
                    changed,
                    "Нужный источник отсутствует в настройках GNOME или экран заблокирован"
                );
            }
            Driver::Kde => {
                let layouts: Vec<(String, String, String)> = proxy.call("getLayoutsList", &())?;
                let index = layouts
                    .iter()
                    .position(|row| language(&row.0) == Some(layout))
                    .context("RU/EN отсутствует в настройках Plasma")?;
                ensure!(
                    proxy.call::<_, _, bool>("setLayout", &(index as u32,))?,
                    "Plasma отклонила смену раскладки"
                );
            }
        }
        Ok(())
    }
}
fn language(name: &str) -> Option<Layout> {
    let name = name
        .strip_prefix("keyboard-")
        .unwrap_or(name)
        .split(['+', '-'])
        .next()?;
    match name {
        "us" | "gb" => Some(Layout::En),
        "ru" => Some(Layout::Ru),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_layout_identifiers() {
        assert_eq!(language("keyboard-ru"), Some(Layout::Ru));
        assert_eq!(language("keyboard-us-dvorak"), Some(Layout::En));
        assert_eq!(language("ru+phonetic"), Some(Layout::Ru));
        assert_eq!(language("pinyin"), None);
    }
}
