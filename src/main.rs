//! Точка входа: настройки и трей, выбор одного текстового бэкенда и общий цикл службы.

mod icons;
mod tray;
mod ui;

use anyhow::Result;
use punto_rs::{
    config::{self, Backend, Config},
    desktop,
    dictionary::Dictionaries,
    layout,
    service::{self, Control, State},
};
use std::{
    io::Read,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("Punto RS: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = config::config_path();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => println!(
            "Punto RS — RU ↔ EN\n\n  punto-rs                 Настройки и tray\n  punto-rs --daemon         Нативный ввод и tray\n  punto-rs --ibus           Добровольный движок IBus и tray\n  punto-rs --backends       Доступные способы подключения\n  punto-rs --settings       Настройки и tray\n  punto-rs --test-sound     Проверить встроенный звуковой сигнал\n  punto-rs --check          Проверить настройки и словари\n  punto-rs --convert TEXT   Исправить раскладку текста (без TEXT читает stdin)\n  punto-rs --default-config Показать настройки по умолчанию\n\nУстановка: scripts/install.sh. Подробности: README.md"
        ),
        Some("--version" | "-V") => println!("punto-rs {}", env!("CARGO_PKG_VERSION")),
        Some("--test-sound") => punto_rs::sound::play()?,
        Some("--backends") => println!("{}", punto_rs::setup::diagnostics()?),
        Some("--convert") => {
            let mut text = args[1..].join(" ");
            if args.len() == 1 {
                std::io::stdin().read_to_string(&mut text)?;
            }
            print!("{}", layout::convert_selection(&text));
        }
        Some("--default-config") => print!("{}", toml::to_string_pretty(&Config::default())?),
        Some("--check") => {
            let config = Config::load(&path)?;
            let dictionaries = Dictionaries::load(&config)?;
            anyhow::ensure!(
                dictionaries.check("hello", layout::Layout::En, &config),
                "Английский словарь не распознал hello"
            );
            anyhow::ensure!(
                dictionaries.check("привет", layout::Layout::Ru, &config),
                "Русский словарь не распознал привет"
            );
            println!("Настройки и словари исправны: {}", path.display());
        }
        Some("--ibus" | "--daemon") => daemon(path, args[0] == "--ibus")?,
        Some("--settings-window") => ui::run(path)?,
        None | Some("--settings") => {
            if !service::running() {
                Command::new(std::env::current_exe()?)
                    .arg("--daemon")
                    .stdin(std::process::Stdio::null())
                    .spawn()?;
            }
            ui::run(path)?;
        }
        Some(other) => anyhow::bail!("Неизвестный аргумент: {other}; см. --help"),
    }
    Ok(())
}

fn daemon(path: std::path::PathBuf, force_ibus: bool) -> Result<()> {
    use ksni::blocking::TrayMethods;
    let config = Config::load(&path)?;
    let dictionaries = Dictionaries::load(&config)?;
    let shared = Arc::new(Mutex::new(State::new(config, path, dictionaries)));
    let show = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(AtomicBool::new(false));
    let _control = service::control_connection(Control {
        shared: shared.clone(),
        show: show.clone(),
        quit: quit.clone(),
    })?;
    let mut ibus = None;
    let mut native = None;
    let mut active_ibus = None;
    let mut next_connection = std::time::Instant::now();
    desktop::watch(shared.clone());
    let tray = match (tray::Tray {
        shared: shared.clone(),
        show: show.clone(),
        quit: quit.clone(),
        icon_directory: tray::icon_directory()?,
    })
    .spawn()
    {
        Ok(handle) => Some(handle),
        Err(error) => {
            shared.lock().unwrap().message =
                format!("Tray недоступен: {error}. В GNOME включите AppIndicator.");
            None
        }
    };
    let mut status = (
        layout::Layout::En,
        false,
        false,
        String::new(),
        String::new(),
    );
    while !quit.load(Ordering::Relaxed) {
        if _control.is_closed() {
            break;
        }
        let want_ibus = force_ibus || shared.lock().unwrap().config.backend == Backend::Ibus;
        if active_ibus != Some(want_ibus) {
            ibus = None;
            native = None;
            let mut state = shared.lock().unwrap();
            state.engine = punto_rs::engine::Engine::new(&state.config);
            state.actions.clear();
            state.active_hotkeys.clear();
            state.connect_shortcuts = false;
            state.connected = false;
            state.focused = false;
            state.engine.capabilities = 0;
            state.backend = if want_ibus {
                "IBus"
            } else {
                "Нативный"
            }
            .into();
            next_connection = std::time::Instant::now();
            active_ibus = Some(want_ibus);
        }
        if show.swap(false, Ordering::Relaxed) {
            let binary = std::env::current_exe()?;
            let shared = shared.clone();
            std::thread::spawn(move || {
                if let Err(error) = Command::new(binary).arg("--settings-window").status() {
                    shared.lock().unwrap().message = format!("Настройки: {error}");
                }
            });
        }
        let next = {
            let state = shared.lock().unwrap();
            (
                state.engine.layout,
                state.config.automatic,
                state.automatic(),
                state.engine.app.clone(),
                state.message.clone(),
            )
        };
        if next != status {
            if let Some(tray) = &tray {
                tray.update(|_| {});
            }
            status = next;
        }
        if want_ibus
            && ibus
                .as_ref()
                .is_some_and(zbus::blocking::Connection::is_closed)
        {
            ibus = None;
            let mut state = shared.lock().unwrap();
            state.connected = false;
            state.engine.capabilities = 0;
        }
        if want_ibus && ibus.is_none() && std::time::Instant::now() >= next_connection {
            match service::connect_ibus(shared.clone()) {
                Ok(connection) => {
                    ibus = Some(connection);
                    let mut state = shared.lock().unwrap();
                    if state.message.starts_with("IBus:") {
                        state.message.clear();
                    }
                }
                Err(error) => {
                    shared.lock().unwrap().message =
                        format!("IBus: {error}. Можно выбрать нативный режим в настройках.")
                }
            }
            next_connection = std::time::Instant::now() + Duration::from_secs(2);
        }
        if !want_ibus {
            if native.is_none() && std::time::Instant::now() >= next_connection {
                match punto_rs::native::Native::new(&shared) {
                    Ok(backend) => native = Some(backend),
                    Err(error) => {
                        shared.lock().unwrap().message = format!("Нативный ввод: {error:#}")
                    }
                }
                next_connection = std::time::Instant::now() + Duration::from_secs(2);
            }
            if let Some(backend) = &mut native
                && let Err(error) = backend.poll(&shared)
            {
                native = None;
                let mut state = shared.lock().unwrap();
                state.message = format!("Нативный ввод: {error:#}");
                state.connected = false;
                state.focused = false;
                state.engine.capabilities = 0;
                state.actions.clear();
                next_connection = std::time::Instant::now() + Duration::from_secs(2);
            }
        }
        std::thread::sleep(Duration::from_millis(if want_ibus { 150 } else { 10 }));
    }
    Ok(())
}
