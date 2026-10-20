//! Контекст активного приложения и блокировки для правил; неизвестный контекст не считается разрешением.

use std::time::Duration;
use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt},
};

#[derive(Clone, Debug, Default)]
pub struct Focus {
    pub app: String,
    pub title: String,
    pub locked: bool,
    pub known: bool,
}

pub fn read() -> Focus {
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
        return read_gnome().unwrap_or_default();
    }
    read_x11().unwrap_or_default()
}

fn read_gnome() -> anyhow::Result<Focus> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_millis(300))
        .build()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.punto.Switcher.Focus",
        "/org/punto/Switcher/Focus",
        "org.punto.Switcher.Focus",
    )?;
    let (app, title, locked): (String, String, bool) = proxy.call("GetFocus", &())?;
    Ok(Focus {
        app,
        title,
        locked,
        known: true,
    })
}

fn read_x11() -> anyhow::Result<Focus> {
    let (connection, screen) = x11rb::connect(None)?;
    let atom = |name: &str| -> anyhow::Result<u32> {
        Ok(connection
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom)
    };
    let window = connection
        .get_property(
            false,
            connection.setup().roots[screen].root,
            atom("_NET_ACTIVE_WINDOW")?,
            AtomEnum::WINDOW,
            0,
            1,
        )?
        .reply()?
        .value32()
        .and_then(|mut v| v.next())
        .filter(|id| *id != 0)
        .ok_or_else(|| anyhow::anyhow!("Нет активного окна"))?;
    let class = connection
        .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)?
        .reply()?
        .value;
    let app = String::from_utf8_lossy(&class).replace('\0', " ");
    let title = connection
        .get_property(
            false,
            window,
            atom("_NET_WM_NAME")?,
            atom("UTF8_STRING")?,
            0,
            4096,
        )?
        .reply()?
        .value;
    Ok(Focus {
        app,
        title: String::from_utf8_lossy(&title).into_owned(),
        locked: false,
        known: true,
    })
}

pub fn watch(shared: crate::service::Shared) {
    std::thread::spawn(move || {
        loop {
            let focus = read();
            if let Ok(mut state) = shared.lock() {
                state.desktop = focus;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    });
}
