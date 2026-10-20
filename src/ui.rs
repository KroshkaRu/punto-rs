//! Графические настройки и поле проверки с теми же словарями; GUI не участвует в глобальном перехвате.

use eframe::egui;
use punto_rs::{
    autostart,
    config::{AppRule, Backend, Config},
    dictionary::Dictionaries,
    layout::{self, Layout},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Settings {
    icons: crate::icons::Icons,
    config: Config,
    path: PathBuf,
    autostart: bool,
    tab: usize,
    message: String,
    status: String,
    rule_input: String,
    snippet_input: String,
    english_input: String,
    russian_input: String,
    ignored_input: String,
    scratch: String,
    dictionaries: Result<Dictionaries, String>,
    last_status: std::time::Instant,
    setup_result: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

impl Settings {
    fn new(path: PathBuf, ctx: &egui::Context) -> Self {
        let (config, mut message) = match Config::load(&path) {
            Ok(c) => (c, String::new()),
            Err(e) => (Config::default(), e.to_string()),
        };
        let autostart = autostart::enabled(&autostart::path()).unwrap_or_else(|error| {
            message = error.to_string();
            false
        });
        let dictionaries = Dictionaries::load(&config).map_err(|error| error.to_string());
        Self {
            icons: crate::icons::Icons::load(ctx),
            rule_input: map_text(&config.rules),
            snippet_input: map_text(&config.replacements),
            english_input: config.english_words.join("\n"),
            russian_input: config.russian_words.join("\n"),
            ignored_input: config.ignored_words.join("\n"),
            config,
            path,
            autostart,
            tab: 0,
            message,
            status: String::new(),
            scratch: String::new(),
            dictionaries,
            last_status: std::time::Instant::now() - Duration::from_secs(2),
            setup_result: None,
        }
    }
    fn save(&mut self) -> anyhow::Result<()> {
        let mut config = self.config.clone();
        config.rules = parse_map(&self.rule_input)?;
        config.replacements = parse_map(&self.snippet_input)?;
        config.english_words = lines(&self.english_input);
        config.russian_words = lines(&self.russian_input);
        config.ignored_words = lines(&self.ignored_input);
        config.validate()?;
        let dictionaries = Dictionaries::load(&config)?;
        config.save(&self.path)?;
        autostart::set_enabled(&autostart::path(), self.autostart)?;
        self.config = config;
        self.dictionaries = Ok(dictionaries);
        if let Ok(connection) = zbus::blocking::connection::Builder::session()?
            .method_timeout(Duration::from_millis(700))
            .build()
            && zbus::blocking::fdo::DBusProxy::new(&connection)?
                .name_has_owner("org.punto.Switcher".try_into()?)?
            && let Ok(proxy) = zbus::blocking::Proxy::new(
                &connection,
                "org.punto.Switcher",
                "/org/punto/Switcher",
                "org.punto.Switcher",
            )
        {
            let _: () = proxy.call("Reload", &())?;
        }
        Ok(())
    }
    fn general(&mut self, ui: &mut egui::Ui) {
        ui.heading("Переключение русского и английского");
        ui.checkbox(
            &mut self.config.automatic,
            "Автоматически исправлять раскладку после завершения слова",
        );
        ui.checkbox(
            &mut self.config.remember_layout,
            "Запоминать раскладку для каждого приложения",
        );
        ui.checkbox(
            &mut self.config.skip_uppercase,
            "Не исправлять АББРЕВИАТУРЫ",
        );
        ui.checkbox(&mut self.config.sound, "Звуковой сигнал при исправлении");
        ui.checkbox(&mut self.autostart, "Запускать при входе в систему");
        ui.horizontal(|ui| {
            ui.label("Мин. длина прочих слов:").on_hover_text("Слова из 1–2 букв проверяются независимо от порога, если исходный вариант неизвестен словарю.");
            ui.add(egui::DragValue::new(&mut self.config.minimum_word).range(2..=32));
        });
        ui.horizontal(|ui| {
            ui.label("Начальная раскладка:");
            ui.radio_value(&mut self.config.initial_layout, Layout::En, "EN");
            ui.radio_value(&mut self.config.initial_layout, Layout::Ru, "RU");
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Обработка ввода:");
            egui::ComboBox::from_id_salt("backend")
                .selected_text(match self.config.backend {
                    Backend::Auto => "Автоматически",
                    Backend::Native => "Нативный (без IBus)",
                    Backend::Ibus => "IBus (уже настроенный)",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.config.backend, Backend::Auto, "Автоматически");
                    ui.selectable_value(
                        &mut self.config.backend,
                        Backend::Native,
                        "Нативный (без IBus)",
                    );
                    ui.selectable_value(
                        &mut self.config.backend,
                        Backend::Ibus,
                        "IBus (уже настроенный)",
                    );
                });
        });
        ui.label("Сохранение применяет выбранный режим. Системные RU/EN, Ctrl+Shift и метод ввода не перенастраиваются.");
        if self.config.backend == Backend::Ibus {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.setup_result.is_none(),
                        egui::Button::new("Выбрать Punto RS в IBus"),
                    )
                    .clicked()
                {
                    let (send, receive) = std::sync::mpsc::channel();
                    self.setup_result = Some(receive);
                    std::thread::spawn(move || {
                        let _ = send.send(
                            punto_rs::setup::configure().map_err(|error| format!("{error:#}")),
                        );
                    });
                }
                if self.setup_result.is_some() {
                    ui.spinner();
                    ui.label("Подключение IBus…");
                }
            });
            ui.label("IBus используется только по вашему выбору. Кнопка выбирает движок в уже запущенном IBus; службы и системные настройки не меняются.");
        } else {
            ui.label("Нативный режим использует AT-SPI: нужны доступные Text и EditableText. Защищённые и недоступные поля пропускаются. В X11 горячие клавиши работают через XKB, обычное переключение языков сохраняется.");
        }
        ui.separator();
        ui.label("Проверка автопереключения: руддщ + пробел = hello");
        let mut edit = egui::TextEdit::multiline(&mut self.scratch)
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .show(ui);
        if edit.response.changed()
            && self.config.automatic
            && let Some(cursor) = edit.cursor_range.filter(|range| range.is_empty())
            && ui.input(|input| input.events.iter().any(completes_word))
            && let Ok(dictionaries) = &self.dictionaries
            && let Some((start, word, tail)) =
                layout::completed_word(&self.scratch, cursor.primary.index)
            && let Some(correction) = dictionaries.replacement(
                &word,
                layout::word_layout(&word, self.config.initial_layout),
                &self.config,
            )
        {
            self.scratch = self
                .scratch
                .chars()
                .take(start)
                .chain(correction.text.chars())
                .chain(tail.chars())
                .chain(self.scratch.chars().skip(cursor.primary.index))
                .collect();
            let cursor = egui::text::CCursor::new(start + correction.text.chars().count() + 1);
            edit.state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
            edit.state.store(ui.ctx(), edit.response.id);
            if let Err(error) = switch_layout(correction.layout) {
                self.message = format!("Слово исправлено; раскладка не переключена: {error}");
            }
            ui.ctx().request_repaint();
        }
        if let Err(error) = &self.dictionaries {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else if !self.config.automatic {
            ui.label("Автопереключение выключено галочкой выше.");
        }
        if ui.button("RU ↔ EN").clicked() {
            self.scratch = layout::convert_selection(&self.scratch);
        }
        ui.label("Пробел или Enter завершает слово. После исправления язык набора переключается.");
    }
    fn hotkeys(&mut self, ui: &mut egui::Ui) {
        ui.heading("Горячие клавиши");
        ui.label("Формат: Ctrl+Alt+S, Pause, ScrollLock, Shift+F12. В X11 сочетания глобальные; занятые системой не перехватываются. В IBus действуют в поле ввода выбранного источника Punto RS.");
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
            && self.config.backend != Backend::Ibus
        {
            ui.label("Для Wayland сохраните сочетания и разрешите их через портал рабочего стола. После изменения сочетаний подключите их повторно.");
            if ui
                .button("Разрешить глобальные сочетания (Wayland)")
                .clicked()
            {
                self.message = match connect_shortcuts() {
                    Ok(()) => "Откройте диалог разрешения рабочего стола".into(),
                    Err(error) => error.to_string(),
                };
            }
        }
        egui::Grid::new("hotkeys")
            .spacing([24., 10.])
            .show(ui, |ui| {
                for binding in &mut self.config.hotkeys {
                    ui.horizontal(|ui| {
                        let icon = if binding.action == punto_rs::config::Action::Undo {
                            &self.icons.undo
                        } else {
                            &self.icons.keyboard
                        };
                        ui.add(
                            egui::Image::new((icon.id(), egui::vec2(18., 18.)))
                                .tint(ui.visuals().text_color()),
                        );
                        ui.label(binding.action.label());
                    });
                    ui.text_edit_singleline(&mut binding.keys);
                    ui.end_row();
                }
            });
    }
    fn exceptions(&mut self, ui: &mut egui::Ui) {
        ui.heading("Правила для приложений");
        ui.label("Подстроки имени приложения и заголовка, без учёта регистра. Если заданы оба условия, должны совпасть оба.");
        ui.label("В GNOME Wayland нужен мост из gnome-extension/. При неизвестном приложении автокоррекция отключена.");
        let mut remove = None;
        for (index, rule) in self.config.exceptions.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("Приложение:");
                        ui.text_edit_singleline(&mut rule.app);
                        if ui.button("Удалить").clicked() {
                            remove = Some(index);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Заголовок:");
                        ui.text_edit_singleline(&mut rule.title);
                    });
                    ui.checkbox(&mut rule.disable_auto, "Без автопереключения");
                    ui.horizontal(|ui| {
                        ui.label("Раскладка:");
                        ui.radio_value(&mut rule.layout, None, "Любая");
                        ui.radio_value(&mut rule.layout, Some(Layout::En), "EN");
                        ui.radio_value(&mut rule.layout, Some(Layout::Ru), "RU");
                    });
                });
            });
        }
        if let Some(index) = remove {
            self.config.exceptions.remove(index);
        }
        if ui.button("Добавить программу").clicked() {
            self.config.exceptions.push(AppRule {
                disable_auto: true,
                ..Default::default()
            });
        }
    }
    fn dictionaries(&mut self, ui: &mut egui::Ui) {
        ui.heading("Словари и правила");
        ui.label("Словари Hunspell .aff + .dic, путь без расширения. Учитываются словоформы.");
        path_edit(ui, "Английский", &mut self.config.english_dictionary);
        path_edit(ui, "Русский", &mut self.config.russian_dictionary);
        ui.separator();
        ui.label("Пользовательские слова: по одному на строку.");
        ui.columns(3, |columns| {
            columns[0].label("Английские");
            columns[0].add(egui::TextEdit::multiline(&mut self.english_input).desired_rows(8));
            columns[1].label("Русские");
            columns[1].add(egui::TextEdit::multiline(&mut self.russian_input).desired_rows(8));
            columns[2].label("Не исправлять");
            columns[2].add(egui::TextEdit::multiline(&mut self.ignored_input).desired_rows(8));
        });
        ui.label("Принудительные правила: исходное = исправленное; регистр учитывается.");
        ui.add(
            egui::TextEdit::multiline(&mut self.rule_input)
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );
    }
    fn snippets(&mut self, ui: &mut egui::Ui) {
        ui.heading("Автозамена");
        ui.label("По одному правилу на строку: сокращение = полный текст. Для новой строки используйте \\n. Срабатывает вместе с автопереключением, отменяется той же клавишей отката.");
        ui.add(
            egui::TextEdit::multiline(&mut self.snippet_input)
                .desired_rows(18)
                .desired_width(f32::INFINITY),
        );
    }
}

impl eframe::App for Settings {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(result) = self
            .setup_result
            .as_ref()
            .and_then(|receive| receive.try_recv().ok())
        {
            self.message = match result {
                Ok(message) => message,
                Err(error) => format!("Ошибка: {error}"),
            };
            self.setup_result = None;
        }
        if self.last_status.elapsed() > Duration::from_secs(2) {
            self.last_status = std::time::Instant::now();
            self.status = status().unwrap_or_else(|| {
                "Приложение запускается; системные настройки ввода не меняются.".into()
            });
        }
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.image((self.icons.app.id(), egui::vec2(26., 26.)));
                ui.heading("Punto RS");
                ui.label("Русский ↔ English");
            });
            ui.label(&self.status);
        });
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Сохранить").clicked() {
                    self.message = match self.save() {
                        Ok(()) => "Настройки сохранены".into(),
                        Err(e) => format!("Ошибка: {e:#}"),
                    };
                }
                ui.add(egui::Label::new(&self.message).wrap());
            });
            ui.small(format!("Настройки: {}", self.path.display()));
        });
        egui::SidePanel::left("tabs")
            .resizable(false)
            .default_width(175.)
            .show(ctx, |ui| {
                for (index, title) in [
                    "Общие",
                    "Горячие клавиши",
                    "Программы-исключения",
                    "Словари",
                    "Автозамена",
                ]
                .iter()
                .enumerate()
                {
                    let icon = match index {
                        1 => &self.icons.keyboard,
                        3 => &self.icons.translate,
                        4 => &self.icons.swap,
                        _ => &self.icons.settings,
                    };
                    let image = egui::Image::new((icon.id(), egui::vec2(18., 18.)))
                        .tint(ui.visuals().text_color());
                    if ui
                        .add(
                            egui::Button::image_and_text(image, *title).selected(self.tab == index),
                        )
                        .clicked()
                    {
                        self.tab = index;
                    }
                }
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                0 => self.general(ui),
                1 => self.hotkeys(ui),
                2 => self.exceptions(ui),
                3 => self.dictionaries(ui),
                _ => self.snippets(ui),
            });
        });
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
fn map_text(map: &BTreeMap<String, String>) -> String {
    map.iter()
        .map(|(a, b)| format!("{a} = {}", b.replace('\\', "\\\\").replace('\n', "\\n")))
        .collect::<Vec<_>>()
        .join("\n")
}
fn parse_map(text: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        let (from, to) = line
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("Правило без знака =: {line}"))?;
        let mut value = String::new();
        let mut chars = to.trim().chars();
        while let Some(ch) = chars.next() {
            if ch != '\\' {
                value.push(ch);
            } else {
                match chars.next() {
                    Some('n') => value.push('\n'),
                    Some('\\') => value.push('\\'),
                    _ => anyhow::bail!("Допустимы только \\n и \\\\: {line}"),
                }
            }
        }
        if map.insert(from.trim().to_string(), value).is_some() {
            anyhow::bail!("Повтор сокращения: {from}");
        }
    }
    Ok(map)
}
fn path_edit(ui: &mut egui::Ui, label: &str, path: &mut PathBuf) {
    let mut text = path.to_string_lossy().into_owned();
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.text_edit_singleline(&mut text).changed() {
            *path = text.into();
        }
    });
}
fn status() -> Option<String> {
    let connection = zbus::blocking::connection::Builder::session()
        .ok()?
        .method_timeout(Duration::from_millis(500))
        .build()
        .ok()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.punto.Switcher",
        "/org/punto/Switcher",
        "org.punto.Switcher",
    )
    .ok()?;
    let (layout, auto, app, message): (String, bool, String, String) =
        proxy.call("Status", &()).ok()?;
    let backend: String = proxy.call("InputBackend", &()).unwrap_or_default();
    Some(format!(
        "{backend} · {layout} · {} · {app}  {message}",
        if auto { "Авто" } else { "Вручную" }
    ))
}

struct WindowControl {
    context: Arc<Mutex<Option<egui::Context>>>,
}
#[zbus::interface(name = "org.punto.Switcher.Settings")]
impl WindowControl {
    fn present(&self) {
        if let Some(ctx) = &*self.context.lock().unwrap() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    }
}
pub fn run(path: PathBuf) -> anyhow::Result<()> {
    let context = Arc::new(Mutex::new(None));
    let control = zbus::blocking::connection::Builder::session()?
        .serve_at(
            "/org/punto/Switcher/Settings",
            WindowControl {
                context: context.clone(),
            },
        )?
        .name("org.punto.Switcher.Settings")?
        .build();
    let _control = match control {
        Ok(c) => c,
        Err(_) => {
            let connection = zbus::blocking::Connection::session()?;
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                "org.punto.Switcher.Settings",
                "/org/punto/Switcher/Settings",
                "org.punto.Switcher.Settings",
            )?;
            let _: () = proxy.call("Present", &())?;
            return Ok(());
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Punto RS — настройки")
            .with_app_id("punto-rs")
            .with_icon(crate::icons::app_icon())
            .with_inner_size([900., 650.])
            .with_min_inner_size([700., 500.]),
        ..Default::default()
    };
    eframe::run_native(
        "Punto RS",
        options,
        Box::new(move |cc| {
            *context.lock().unwrap() = Some(cc.egui_ctx.clone());
            Ok(Box::new(Settings::new(path, &cc.egui_ctx)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Окно настроек: {e}"))
}

fn connect_shortcuts() -> anyhow::Result<()> {
    let connection = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.punto.Switcher",
        "/org/punto/Switcher",
        "org.punto.Switcher",
    )?;
    Ok(proxy.call("ConnectShortcuts", &())?)
}

fn completes_word(event: &egui::Event) -> bool {
    match event {
        egui::Event::Text(text) | egui::Event::Paste(text) => {
            text.chars().last().is_some_and(layout::delimiter)
        }
        egui::Event::Key {
            key: egui::Key::Enter | egui::Key::Tab,
            pressed: true,
            ..
        } => true,
        _ => false,
    }
}

fn switch_layout(layout: Layout) -> anyhow::Result<()> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_secs(2))
        .build()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.punto.Switcher",
        "/org/punto/Switcher",
        "org.punto.Switcher",
    )?;
    Ok(proxy.call("SetLayout", &(layout.label(),))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_new_input_ending_in_a_separator_triggers_preview() {
        assert!(completes_word(&egui::Event::Text(" ".into())));
        assert!(completes_word(&egui::Event::Text("щ ".into())));
        assert!(completes_word(&egui::Event::Paste("руддщ ".into())));
        assert!(!completes_word(&egui::Event::Text("руддщ".into())));
        assert!(!completes_word(&egui::Event::Copy));
        for (key, expected) in [
            (egui::Key::Enter, true),
            (egui::Key::Backspace, false),
            (egui::Key::Z, false),
        ] {
            assert_eq!(
                completes_word(&egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }),
                expected
            );
        }
    }
    #[test]
    fn snippet_editor_roundtrip() {
        let mut map = BTreeMap::new();
        map.insert("sig".into(), "Hello\nC:\\new".into());
        assert_eq!(parse_map(&map_text(&map)).unwrap(), map);
        assert!(parse_map("a=x\na=y").is_err());
        assert!(parse_map("bad").is_err());
    }
}
