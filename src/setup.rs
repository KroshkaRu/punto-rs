//! Выбор Punto в уже работающем IBus; без запуска IBus и изменения системного метода ввода.

use anyhow::{Context, Result, bail};
use std::{
    process::Command,
    time::{Duration, Instant},
};

pub fn configure() -> Result<String> {
    let control_connection = zbus::blocking::Connection::session()?;
    let control = zbus::blocking::Proxy::new(
        &control_connection,
        "org.punto.Switcher",
        "/org/punto/Switcher",
        "org.punto.Switcher",
    )?;
    if control.call::<_, _, String>("InputBackend", &())? != "IBus" {
        bail!("Сначала сохраните режим IBus в настройках.");
    }
    let address = Command::new("ibus")
        .arg("address")
        .output()
        .context("IBus не установлен; можно использовать нативный режим")?;
    let address = String::from_utf8_lossy(&address.stdout);
    if address.trim().is_empty() || address.trim() == "(null)" {
        bail!(
            "IBus не запущен. Нативный режим работает без IBus. При желании настройте IBus средствами рабочего стола."
        );
    }
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_secs(1))
        .build()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.punto.Switcher",
        "/org/punto/Switcher",
        "org.punto.Switcher",
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !proxy.call::<_, _, bool>("InputReady", &())? {
        if Instant::now() >= deadline {
            bail!("Выберите режим IBus и сохраните настройки перед выбором движка.");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let output = Command::new("ibus").args(["engine", "punto-rs"]).output()?;
    if !output.status.success() {
        bail!(
            "Выбор Punto RS: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok("Punto RS выбран в уже запущенном IBus. Настройки системных раскладок и способ ввода не изменялись.".into())
}

pub fn diagnostics() -> Result<String> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_millis(700))
        .build()?;
    let bus = zbus::blocking::fdo::DBusProxy::new(&connection)?;
    let active = bus.list_names()?;
    let activatable = bus.list_activatable_names()?;
    let running = |name: &str| active.iter().any(|item| item.as_str() == name);
    let available =
        |name: &str| running(name) || activatable.iter().any(|item| item.as_str() == name);
    let x11 = std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland")
        && std::env::var_os("DISPLAY").is_some();
    let mut rows = vec![
        format!("Сеанс: {}", if x11 { "X11" } else { "Wayland/без X11" }),
        format!(
            "AT-SPI: {}",
            if available("org.a11y.Bus") {
                "доступен (поля проверяются отдельно)"
            } else {
                "недоступен"
            }
        ),
        format!(
            "X11/XKB: {}",
            if x11 {
                "доступен; системные раскладки не перенастраиваются"
            } else {
                "не используется для Wayland"
            }
        ),
    ];
    for (name, label) in [
        ("org.freedesktop.IBus", "IBus"),
        ("org.fcitx.Fcitx5", "Fcitx5"),
        ("org.kde.keyboard", "Plasma layouts"),
        ("org.punto.Switcher.Focus", "Мост GNOME"),
    ] {
        rows.push(format!(
            "{label}: {}",
            if running(name) {
                "запущен"
            } else {
                "не запущен (автозапуск не выполняется)"
            }
        ));
    }
    let portal = if running("org.freedesktop.portal.Desktop") {
        zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.GlobalShortcuts",
        )?
        .get_property::<u32>("version")
        .is_ok()
    } else {
        false
    };
    rows.push(format!(
        "GlobalShortcuts portal: {}",
        if portal {
            "доступен; подключение по кнопке в настройках"
        } else {
            "не подтверждён"
        }
    ));
    Ok(rows.join("\n"))
}
