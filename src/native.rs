//! Редактирование через AT-SPI: роль поля, снимок текста и проверяемая замена без буфера обмена.

use crate::{
    config::Action,
    engine::Surrounding,
    layout::{self, Layout, completed_word, delimiter, word_layout, word_range},
    service::Shared,
    x11::Keyboard,
};
use anyhow::{Context, Result, bail, ensure};
use atspi_proxies::{
    accessible::AccessibleProxyBlocking,
    common::{
        Interface, ObjectRef, Role, State,
        events::object::{StateChangedEvent, TextCaretMovedEvent, TextChangedEvent},
    },
    editable_text::EditableTextProxyBlocking,
    text::TextProxyBlocking,
};
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use zbus::{
    blocking::{Connection, MessageIterator, Proxy},
    proxy::CacheProperties,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target(ObjectRef);
impl Target {
    fn accessible<'a>(&'a self, connection: &'a Connection) -> Result<AccessibleProxyBlocking<'a>> {
        Ok(AccessibleProxyBlocking::builder(connection)
            .destination(self.0.name.as_str())?
            .path(self.0.path.as_str())?
            .cache_properties(CacheProperties::No)
            .build()?)
    }
    fn text<'a>(&'a self, connection: &'a Connection) -> Result<TextProxyBlocking<'a>> {
        Ok(TextProxyBlocking::builder(connection)
            .destination(self.0.name.as_str())?
            .path(self.0.path.as_str())?
            .cache_properties(CacheProperties::No)
            .build()?)
    }
    fn own_settings(&self, connection: &Connection) -> bool {
        // Settings has its own preview engine; do not edit it twice or call back into its UI thread.
        let pid = zbus::blocking::fdo::DBusProxy::new(connection)
            .ok()
            .and_then(|bus| {
                bus.get_connection_unix_process_id(self.0.name.clone().into())
                    .ok()
            });
        pid.and_then(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).ok())
            .is_some_and(|exe| exe.file_name().is_some_and(|name| name == "punto-rs"))
    }
    fn safe(&self, connection: &Connection) -> Result<bool> {
        if self.own_settings(connection) {
            return Ok(false);
        }
        let accessible = self.accessible(connection)?;
        if accessible.get_role()? == Role::PasswordText {
            return Ok(false);
        }
        let states = accessible.get_state()?;
        let interfaces = accessible.get_interfaces()?;
        let x11 = std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland")
            && std::env::var_os("DISPLAY").is_some();
        Ok((interfaces.contains(Interface::EditableText) || x11)
            && states.contains(State::Focused)
            && states.contains(State::Editable)
            && !states.contains(State::Defunct)
            && interfaces.contains(Interface::Text))
    }
    fn snapshot(&self, connection: &Connection) -> Result<Surrounding> {
        ensure!(
            self.safe(connection)?,
            "Поле не поддерживает безопасное редактирование или защищено"
        );
        let text = self.text(connection)?;
        let count = text.character_count()?;
        ensure!(
            (0..=262144).contains(&count),
            "Поле слишком велико для безопасного преобразования"
        );
        let value = text.get_text(0, count)?;
        let cursor = usize::try_from(text.caret_offset()?)?;
        // atspi-proxies 0.9 generates GetNselections with the wrong capital S.
        let selections: i32 = text.inner().call("GetNSelections", &())?;
        let anchor = if selections == 1 {
            // shortcut: Chromium reports wrong selection offsets after non-BMP text; refuse until its ATK API is fixed.
            ensure!(
                self.accessible(connection)?
                    .get_interfaces()?
                    .contains(Interface::EditableText)
                    || !value.chars().any(|ch| ch as u32 > 0xffff),
                "Нельзя достоверно прочитать выделение Chromium после символов вне BMP"
            );
            let (start, end) = text.get_selection(0)?;
            if cursor == start as usize {
                end as usize
            } else {
                start as usize
            }
        } else {
            cursor
        };
        let snapshot = Surrounding {
            text: value,
            cursor,
            anchor,
        };
        ensure!(
            snapshot.valid(),
            "Приложение вернуло неверные границы текста"
        );
        Ok(snapshot)
    }
}

struct Undo {
    event_serial: u32,
    target: Target,
    expected: Surrounding,
    start: usize,
    replacement_len: usize,
    original: String,
    layout: Layout,
}

struct ShortcutFocus {
    window: u32,
    target: Option<Target>,
    pointer_revision: u64,
}

pub struct Native {
    connection: Connection,
    events: Receiver<zbus::Message>,
    target: Option<Target>,
    undo: Option<Undo>,
    last_edit: Option<(Target, Surrounding)>,
    keyboard: Option<Keyboard>,
    last_keyboard: Instant,
    portal: Option<crate::portal::Portal>,
    shortcuts_ready: Instant,
    shortcut_focus: Option<ShortcutFocus>,
    session_layout: Option<crate::session_layout::SessionLayout>,
    input: Option<crate::x11_input::InputBackend>,
    target_pid: Option<u32>,
}
impl Native {
    pub fn new(shared: &Shared) -> Result<Self> {
        let session = zbus::blocking::connection::Builder::session()?
            .method_timeout(Duration::from_secs(1))
            .build()?;
        let address: String =
            Proxy::new(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus")?
                .call("GetAddress", &())?;
        let connection = zbus::blocking::connection::Builder::address(address.as_str())?
            .method_timeout(Duration::from_millis(500))
            .build()?;
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.a11y.atspi.Event.Object")?
            .build();
        let iterator = MessageIterator::for_match_rule(rule, &connection, Some(256))?;
        let registry = Proxy::new(
            &connection,
            "org.a11y.atspi.Registry",
            "/org/a11y/atspi/registry",
            "org.a11y.atspi.Registry",
        )?;
        let xml: String = Proxy::new(
            &connection,
            "org.a11y.atspi.Registry",
            "/org/a11y/atspi/registry",
            "org.freedesktop.DBus.Introspectable",
        )?
        .call("Introspect", &())?;
        // Ubuntu ships different RegisterEvent argument lists; discover the live API.
        let signature = registry_signature(&xml)?;
        for name in [
            "object:state-changed:focused",
            "object:text-changed",
            "object:text-caret-moved",
            "object:text-selection-changed",
        ] {
            let _: () = match signature.as_str() {
                "s" => registry.call("RegisterEvent", &(name,))?,
                "sas" => registry.call("RegisterEvent", &(name, Vec::<String>::new()))?,
                "sass" => registry.call("RegisterEvent", &(name, Vec::<String>::new(), ""))?,
                _ => bail!("Неизвестная версия AT-SPI RegisterEvent: {signature}"),
            };
        }
        let (send, events) = mpsc::sync_channel(256);
        std::thread::spawn(move || {
            for message in iterator {
                let Ok(message) = message else { break };
                if send.send(message).is_err() {
                    break;
                }
            }
        });
        let keyboard = if std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland")
            && std::env::var_os("DISPLAY").is_some()
        {
            let mut keyboard = Keyboard::new()?;
            // A conflicting global Enter must not disable the other input paths.
            let _ = keyboard.grab_enter();
            Some(keyboard)
        } else {
            None
        };
        let (input, input_error) = if keyboard.is_some() {
            match crate::x11_input::InputBackend::new() {
                Ok(input) => (Some(input), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "XInput2 недоступен: {error:#}; используется AT-SPI"
                    )),
                ),
            }
        } else {
            (None, None)
        };
        let session_layout = if keyboard.is_none() {
            crate::session_layout::SessionLayout::discover()?
        } else {
            None
        };
        {
            let mut state = shared.lock().unwrap();
            state.connected = true;
            state.backend = if keyboard.is_some() {
                "X11 XInput2 + AT-SPI"
            } else {
                "AT-SPI (Wayland)"
            }
            .into();
            state.engine.capabilities = 0;
            if let Some(layout) = keyboard
                .as_ref()
                .map(Keyboard::layout)
                .transpose()?
                .flatten()
            {
                state.engine.layout = layout;
            }
            state.message = input_error.unwrap_or_else(|| "Ожидание доступного поля ввода".into());
        }
        Ok(Self {
            connection,
            events,
            target: None,
            undo: None,
            last_edit: None,
            keyboard,
            last_keyboard: Instant::now() - Duration::from_secs(1),
            portal: None,
            shortcuts_ready: Instant::now(),
            shortcut_focus: None,
            session_layout,
            input,
            target_pid: None,
        })
    }
    pub fn poll(&mut self, shared: &Shared) -> Result<()> {
        ensure!(!self.connection.is_closed(), "Шина AT-SPI отключилась");
        let connect = std::mem::take(&mut shared.lock().unwrap().connect_shortcuts);
        if connect {
            self.portal = None;
            shared.lock().unwrap().message = "Ожидание разрешения глобальных сочетаний…".into();
            self.portal = Some(crate::portal::Portal::start(shared.clone()));
        }
        if self.last_keyboard.elapsed() >= Duration::from_millis(100) {
            self.last_keyboard = Instant::now();
            if let Some(keyboard) = &mut self.keyboard {
                let mut state = shared.lock().unwrap();
                if std::mem::take(&mut state.rebind_hotkeys) {
                    keyboard.rebind();
                }
                let failures = keyboard.bind(&state.config)?;
                state.active_hotkeys = state
                    .config
                    .hotkeys
                    .iter()
                    .filter(|binding| !binding.keys.is_empty() && !failures.contains(&binding.keys))
                    .map(|binding| binding.keys.clone())
                    .collect();
                if !failures.is_empty() {
                    state.message = format!("Сочетания заняты: {}", failures.join(", "));
                } else if state.message.starts_with("Сочетания заняты:") {
                    state.message.clear();
                }
                if keyboard
                    .reported
                    .is_some_and(|old| old != state.engine.layout)
                {
                    keyboard.switch(state.engine.layout)?;
                }
                if let Some(layout) = keyboard.layout()? {
                    state.engine.layout = layout;
                    keyboard.reported = Some(layout);
                }
            } else if let Some(driver) = &mut self.session_layout {
                let mut state = shared.lock().unwrap();
                let result = (|| -> Result<()> {
                    if driver
                        .reported
                        .is_some_and(|old| old != state.engine.layout)
                    {
                        driver.switch(state.engine.layout)?;
                    }
                    if let Some(layout) = driver.current()? {
                        state.engine.layout = layout;
                        driver.reported = Some(layout);
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    state.message = format!("Раскладка рабочего стола: {error:#}");
                }
            }
        }
        if let Some(keyboard) = &mut self.keyboard {
            let actions = keyboard.actions()?;
            if !actions.is_empty() {
                self.shortcuts_ready = Instant::now() + Duration::from_millis(60);
                if self.shortcut_focus.is_none() {
                    let (window, pid, _, _) = keyboard.window_context()?;
                    self.shortcut_focus = Some(ShortcutFocus {
                        window,
                        target: self
                            .target
                            .clone()
                            .filter(|_| pid.is_some() && pid == self.target_pid),
                        pointer_revision: self
                            .input
                            .as_ref()
                            .map_or(0, |input| input.pointer_revision),
                    });
                }
                let mut state = shared.lock().unwrap();
                for action in actions {
                    // One held chord must not enqueue autorepeat as multiple edits.
                    if state.actions.len() < 16 && !state.actions.contains(&action) {
                        state.actions.push_back(action);
                    }
                }
            }
        }
        for _ in 0..64 {
            let Ok(message) = self.events.try_recv() else {
                break;
            };
            if let Err(error) = self.event(&message, shared) {
                shared.lock().unwrap().message = format!("AT-SPI: {error:#}");
            }
        }
        if let (Some(input), Some(keyboard)) = (&mut self.input, &mut self.keyboard) {
            if let Err(error) = input.poll(keyboard, self.target_pid, shared) {
                shared.lock().unwrap().message = format!("X11: {error:#}");
            }
            if input.active
                && let Some(layout) = keyboard.layout()?
            {
                keyboard.reported = Some(layout);
                shared.lock().unwrap().engine.layout = layout;
            }
        }
        if let Some(event) = self.keyboard.as_mut().and_then(Keyboard::take_enter) {
            let correction = self.before_enter(&event, shared);
            // Replay must run even if dictionaries, accessibility or editing failed.
            let forwarded = self.keyboard.as_mut().unwrap().finish_enter(event);
            for result in [correction, forwarded] {
                if let Err(error) = result {
                    shared.lock().unwrap().message = format!("Enter: {error:#}");
                }
            }
            if let Some(input) = &mut self.input {
                input.clear();
            }
            self.undo = None;
            self.last_edit = None;
        }
        if self.shortcut_focus.is_some()
            && let Some(keyboard) = &self.keyboard
            && !keyboard.keys_released()?
        {
            // Focus returns after the grab ends, which can outlast a fixed delay.
            self.shortcuts_ready = Instant::now() + Duration::from_millis(60);
        }
        for _ in 0..16 {
            if Instant::now() < self.shortcuts_ready {
                break;
            }
            if let (Some(context), Some(keyboard)) = (&self.shortcut_focus, &self.keyboard)
                && (keyboard.input_focus().ok() != Some(context.window)
                    || context
                        .target
                        .as_ref()
                        .is_some_and(|target| self.target.as_ref() != Some(target))
                    || self
                        .input
                        .as_ref()
                        .is_some_and(|input| input.pointer_revision != context.pointer_revision))
            {
                let mut state = shared.lock().unwrap();
                state.actions.clear();
                state.message =
                    "Фокус или выделение изменились; действие горячей клавиши отменено".into();
                self.shortcut_focus = None;
                break;
            }
            let action = shared.lock().unwrap().actions.pop_front();
            let Some(action) = action else {
                self.shortcut_focus = None;
                break;
            };
            if let Err(error) = self.action(action, shared) {
                shared.lock().unwrap().message = error.to_string();
            }
        }
        Ok(())
    }
    fn before_enter(
        &mut self,
        event: &x11rb::protocol::xproto::KeyPressEvent,
        shared: &Shared,
    ) -> Result<()> {
        let unknown = self.input.as_ref().is_some_and(|input| input.active);
        let recorded = self
            .input
            .as_mut()
            .and_then(|input| input.take_enter_word())
            .unwrap_or_default();
        let original = if unknown {
            recorded
        } else {
            let Some(target) = self.target.clone() else {
                return Ok(());
            };
            self.focus(target.clone(), shared)?;
            let Some(keyboard) = &self.keyboard else {
                return Ok(());
            };
            let (_, pid, _, _) = keyboard.window_context()?;
            if pid.is_none()
                || pid != self.target_pid
                || !target.safe(&self.connection)?
                || recorded.is_empty()
            {
                return Ok(());
            }
            // The renderer may still be applying keys delivered before Enter was frozen.
            let deadline = Instant::now() + Duration::from_millis(60);
            loop {
                let snapshot = target.snapshot(&self.connection)?;
                if snapshot.cursor != snapshot.anchor {
                    return Ok(());
                }
                let current = word_range(&snapshot.text, snapshot.cursor)
                    .filter(|(_, end)| *end == snapshot.cursor)
                    .map(|(start, end)| {
                        snapshot
                            .text
                            .chars()
                            .skip(start)
                            .take(end - start)
                            .collect::<String>()
                    });
                if current.as_deref() == Some(&recorded) {
                    break recorded;
                }
                if Instant::now() >= deadline {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        if original.is_empty() || original.chars().any(char::is_whitespace) {
            return Ok(());
        }
        let keyboard = self.keyboard.as_ref().context("X11 недоступен")?;
        let old_layout = keyboard.layout()?.context("Неизвестная раскладка")?;
        let correction = {
            let state = shared.lock().unwrap();
            if !state.engine.automatic(&state.config) {
                return Ok(());
            }
            state.dictionaries.replacement(
                &original,
                word_layout(&original, old_layout),
                &state.config,
            )
        };
        let Some(correction) = correction.filter(|correction| correction.text != original) else {
            return Ok(());
        };
        keyboard.typing_plan(&original, old_layout)?;
        keyboard.replace_before_enter(
            keyboard.input_focus()?,
            original.chars().count(),
            &correction.text,
            correction.layout,
            event,
        )?;
        let mut state = shared.lock().unwrap();
        state.engine.layout = correction.layout;
        let sound = unknown || state.config.sound;
        drop(state);
        if sound {
            crate::service::play_sound(shared.clone());
        }
        Ok(())
    }
    fn focus(&mut self, target: Target, shared: &Shared) -> Result<()> {
        if self.target.as_ref() != Some(&target) {
            self.target_pid = zbus::blocking::fdo::DBusProxy::new(&self.connection)
                .ok()
                .and_then(|bus| {
                    bus.get_connection_unix_process_id(target.0.name.clone().into())
                        .ok()
                });
        }
        if target.own_settings(&self.connection) {
            let mut state = shared.lock().unwrap();
            state.focused = false;
            state.engine.capabilities = 0;
            state.message = "В настройках работает встроенная проверка".into();
            self.target = None;
            self.undo = None;
            return Ok(());
        }
        if self.target.as_ref() == Some(&target) {
            let safe = target.safe(&self.connection).unwrap_or(false);
            let mut state = shared.lock().unwrap();
            state.focused = safe;
            state.engine.capabilities = if safe { 1 | 32 } else { 0 };
            return Ok(());
        }
        self.undo = None;
        self.last_edit = None;
        let safe = target.safe(&self.connection).unwrap_or(false);
        let app_ref = target.accessible(&self.connection)?.get_application()?;
        let app = Target(app_ref).accessible(&self.connection)?.name()?;
        let focus = crate::desktop::read();
        let mut title = focus.title;
        if title.is_empty() {
            let mut parent = target.clone();
            for _ in 0..16 {
                let Ok(node) = parent.accessible(&self.connection) else {
                    break;
                };
                if matches!(
                    node.get_role(),
                    Ok(Role::Frame | Role::Dialog | Role::Window)
                ) {
                    title = node.name().unwrap_or_default();
                    break;
                }
                let Ok(reference) = node.parent() else { break };
                drop(node);
                parent = Target(reference);
            }
        }
        let app = format!("{app} {}", if focus.known { &focus.app } else { "" });
        let mut state = shared.lock().unwrap();
        let crate::service::State { engine, config, .. } = &mut *state;
        engine.focus_in(target.0.path.as_str(), &app, &title, config);
        engine.capabilities = if safe { 1 | 32 } else { 0 };
        state.focused = safe;
        state.message = if safe {
            String::new()
        } else {
            "Поле защищено или не предоставляет EditableText; преобразование отключено".into()
        };
        self.target = Some(target);
        Ok(())
    }
    fn event(&mut self, message: &zbus::Message, shared: &Shared) -> Result<()> {
        if shared.lock().unwrap().desktop.locked {
            self.undo = None;
            return Ok(());
        }
        if let Ok(event) = TextCaretMovedEvent::try_from(message) {
            if self.last_edit.as_ref().is_some_and(|(target, expected)| {
                target.0 == event.item
                    && target
                        .snapshot(&self.connection)
                        .is_ok_and(|actual| actual == *expected)
            }) {
                return Ok(());
            }
            if self.undo.as_ref().is_some_and(|undo| {
                undo.target.0 == event.item
                    && serial_after(
                        message.primary_header().serial_num().get(),
                        undo.event_serial,
                    )
                    && event.position as usize != undo.expected.cursor
            }) {
                self.undo = None;
            }
            return Ok(());
        }
        if let Ok(event) = StateChangedEvent::try_from(message) {
            if event.state == State::Focused {
                if event.enabled {
                    self.focus(Target(event.item), shared)?;
                } else if self
                    .target
                    .as_ref()
                    .is_some_and(|target| target.0 == event.item)
                {
                    // Passive X11 shortcut grabs briefly remove focus from the same field.
                    let mut state = shared.lock().unwrap();
                    state.focused = false;
                    state.engine.capabilities = 0;
                }
            }
            return Ok(());
        }
        let Ok(event) = TextChangedEvent::try_from(message) else {
            return Ok(());
        };
        let target = Target(event.item);
        if !target.safe(&self.connection).unwrap_or(false) {
            return Ok(());
        }
        self.focus(target.clone(), shared)?;
        let before = target.snapshot(&self.connection)?;
        // Electron publishes our synthetic insertions asynchronously, including the space restored by Undo.
        if self
            .last_edit
            .as_ref()
            .is_some_and(|(edited, expected)| *edited == target && *expected == before)
        {
            return Ok(());
        }
        self.last_edit = None;
        if self
            .undo
            .as_ref()
            .is_some_and(|undo| undo.target != target || undo.expected != before)
        {
            self.undo = None;
        }
        if event.operation != atspi_proxies::common::Operation::Insert
            || event.length != 1
            || !event.text.chars().next().is_some_and(delimiter)
            || event
                .start_pos
                .checked_add(event.length)
                .and_then(|position| usize::try_from(position).ok())
                != Some(before.cursor)
            || before.cursor != before.anchor
        {
            return Ok(());
        }
        let (start, original, correction, layout) = {
            let state = shared.lock().unwrap();
            if !state.engine.automatic(&state.config) {
                return Ok(());
            }
            let Some((start, word, tail)) = completed_word(&before.text, before.cursor) else {
                return Ok(());
            };
            let layout = word_layout(&word, state.engine.layout);
            let Some(correction) = state.dictionaries.replacement(&word, layout, &state.config)
            else {
                return Ok(());
            };
            (start, word + &tail, correction, layout)
        };
        let tail: String = original.chars().rev().take(1).collect();
        let replacement = correction.text + &tail;
        if replacement != original {
            let (expected, event_serial) = self.replace(
                &target,
                &before,
                start..before.cursor,
                &replacement,
                correction.layout,
            )?;
            self.last_edit = Some((target.clone(), expected.clone()));
            self.undo = Some(Undo {
                event_serial,
                target,
                expected,
                start,
                replacement_len: replacement.chars().count(),
                original,
                layout,
            });
            self.finish_replacement(correction.layout, shared)?;
            if shared.lock().unwrap().config.sound {
                crate::service::play_sound(shared.clone());
            }
        }
        Ok(())
    }
    fn replace(
        &self,
        target: &Target,
        before: &Surrounding,
        range: std::ops::Range<usize>,
        replacement: &str,
        final_layout: Layout,
    ) -> Result<(Surrounding, u32)> {
        let (start, end) = (range.start, range.end);
        ensure!(
            start <= end && end <= before.text.chars().count(),
            "Некорректный диапазон замены"
        );
        ensure!(
            target.snapshot(&self.connection)? == *before,
            "Текст или курсор изменились; преобразование отменено"
        );
        if !target
            .accessible(&self.connection)?
            .get_interfaces()?
            .contains(Interface::EditableText)
        {
            return self.replace_keyboard(target, before, start..end, replacement, final_layout);
        }
        let edit = EditableTextProxyBlocking::builder(&self.connection)
            .destination(target.0.name.as_str())?
            .path(target.0.path.as_str())?
            .build()?;
        let length = replacement.chars().count();
        let inserted = splice(&before.text, start, start, replacement);
        // Insert first: an application rejecting insertion must not lose the original text.
        ensure!(
            edit.insert_text(start as i32, replacement, replacement.len() as i32)?,
            "Приложение отклонило вставку"
        );
        let actual = target.snapshot(&self.connection)?;
        ensure!(
            actual.text == inserted,
            "Приложение изменило текст при вставке; исходный фрагмент сохранён"
        );
        if let Some(keyboard) = &self.keyboard {
            keyboard.switch(final_layout)?;
        }
        ensure!(
            edit.delete_text((start + length) as i32, (end + length) as i32)?,
            "Приложение отклонило удаление; исходный фрагмент сохранён"
        );
        let expected = Surrounding {
            text: splice(&before.text, start, end, replacement),
            cursor: start + length,
            anchor: start + length,
        };
        let current = target.snapshot(&self.connection)?;
        ensure!(
            current.text == expected.text,
            "Текст изменился после замены; курсор не перемещён"
        );
        let text = target.text(&self.connection)?;
        let reply = if current.cursor == expected.cursor && current.anchor == expected.anchor {
            text.inner().call_method("GetNSelections", &())?
        } else {
            let reply = text
                .inner()
                .call_method("SetCaretOffset", &(expected.cursor as i32,))?;
            ensure!(
                reply.body().deserialize::<bool>()?,
                "Не удалось восстановить курсор"
            );
            reply
        };
        Ok((expected, reply.primary_header().serial_num().get()))
    }
    fn replace_keyboard(
        &self,
        target: &Target,
        before: &Surrounding,
        range: std::ops::Range<usize>,
        replacement: &str,
        final_layout: Layout,
    ) -> Result<(Surrounding, u32)> {
        let (start, end) = (range.start, range.end);
        let keyboard = self
            .keyboard
            .as_ref()
            .context("Поле без EditableText требует X11; на Wayland используйте IBus")?;
        let plan = keyboard.typing_plan(replacement, final_layout)?;
        let original: String = before.text.chars().skip(start).take(end - start).collect();
        // Restrict backspacing to mapped characters, excluding combining/control sequences.
        keyboard.typing_plan(&original, final_layout)?;
        let deadline = Instant::now() + Duration::from_millis(250);
        while let Err(error) = keyboard.idle() {
            if Instant::now() >= deadline {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        ensure!(
            target.snapshot(&self.connection)? == *before,
            "Текст изменился до начала клавиатурной замены"
        );
        let focus = keyboard.input_focus()?;
        let old_layout = keyboard.layout()?.context("Неизвестна текущая раскладка")?;
        let text = target.text(&self.connection)?;
        ensure!(
            text.inner()
                .call::<_, _, bool>("SetCaretOffset", &(start as i32,))?,
            "Приложение отклонило перемещение курсора"
        );
        let result = (|| -> Result<(Surrounding, u32)> {
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                let snapshot = target.snapshot(&self.connection)?;
                ensure!(
                    snapshot.text == before.text,
                    "Текст изменился при перемещении курсора"
                );
                if snapshot.cursor == start && snapshot.anchor == start {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Приложение не подтвердило положение курсора"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            let mut inserted = String::new();
            for ((layout, code, shift), ch) in plan.into_iter().zip(replacement.chars()) {
                ensure!(
                    keyboard.input_focus()? == focus && target.safe(&self.connection)?,
                    "Фокус изменился; замена остановлена"
                );
                keyboard.idle()?;
                keyboard.switch(layout)?;
                keyboard.type_key(code, shift)?;
                inserted.push(ch);
                self.wait_text(target, &splice(&before.text, start, start, &inserted))?;
            }
            let length = replacement.chars().count();
            ensure!(
                keyboard.input_focus()? == focus,
                "Фокус изменился; исходный текст сохранён"
            );
            let snapshot = target.snapshot(&self.connection)?;
            ensure!(
                snapshot.text == splice(&before.text, start, start, replacement),
                "Текст изменился; исходный фрагмент сохранён"
            );
            ensure!(
                text.inner()
                    .call::<_, _, bool>("SetCaretOffset", &((end + length) as i32,))?,
                "Приложение отклонило перемещение курсора"
            );
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                let current = target.snapshot(&self.connection)?;
                ensure!(
                    current.text == snapshot.text,
                    "Текст изменился перед удалением"
                );
                if current.cursor == end + length && current.anchor == current.cursor {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Приложение не подтвердило положение курсора"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            keyboard.switch(final_layout)?;
            for remaining in (start..end).rev() {
                ensure!(
                    keyboard.input_focus()? == focus && target.safe(&self.connection)?,
                    "Фокус изменился; удаление остановлено"
                );
                keyboard.idle()?;
                let current = target.snapshot(&self.connection)?;
                ensure!(
                    current.cursor == remaining + length + 1
                        && current.anchor == current.cursor
                        && current.text
                            == splice(
                                &splice(&before.text, remaining + 1, end, ""),
                                start,
                                start,
                                replacement
                            ),
                    "Текст или курсор изменились; удаление остановлено"
                );
                keyboard.backspace()?;
                self.wait_text(
                    target,
                    &splice(
                        &splice(&before.text, remaining, end, ""),
                        start,
                        start,
                        replacement,
                    ),
                )?;
            }
            let expected = Surrounding {
                text: splice(&before.text, start, end, replacement),
                cursor: start + length,
                anchor: start + length,
            };
            self.wait_text(target, &expected.text)?;
            // Backspace already leaves the caret here; moving it again can reorder new physical input.
            let reply = text.inner().call_method("GetNSelections", &())?;
            Ok((expected, reply.primary_header().serial_num().get()))
        })();
        if result.is_err() && keyboard.input_focus()? == focus {
            keyboard.switch(old_layout)?;
        }
        result
    }
    fn wait_text(&self, target: &Target, expected: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if target.snapshot(&self.connection)?.text == expected {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Приложение не подтвердило ввод; исходный фрагмент сохранён"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn finish_replacement(&mut self, layout: Layout, shared: &Shared) -> Result<()> {
        if let Some(keyboard) = &mut self.keyboard {
            // X11 selected the final group before deletion; preserve a newer physical Ctrl+Shift.
            let actual = keyboard.layout()?.unwrap_or(layout);
            keyboard.reported = Some(actual);
            shared.lock().unwrap().engine.layout = actual;
            Ok(())
        } else {
            self.set_layout(layout, shared)
        }
    }
    fn set_layout(&mut self, layout: Layout, shared: &Shared) -> Result<()> {
        if let Some(keyboard) = &mut self.keyboard {
            keyboard.switch(layout)?;
            keyboard.reported = Some(layout);
        } else if let Some(driver) = &mut self.session_layout {
            driver.switch(layout)?;
            driver.reported = Some(layout);
        } else {
            bail!(
                "Управление раскладкой Wayland недоступно. Нужен мост GNOME, Plasma или настроенный Fcitx5."
            );
        }
        shared.lock().unwrap().engine.layout = layout;
        Ok(())
    }
    fn action(&mut self, action: Action, shared: &Shared) -> Result<()> {
        ensure!(!shared.lock().unwrap().desktop.locked, "Экран заблокирован");
        match action {
            Action::ToggleAuto | Action::PauseApp => {
                let mut state = shared.lock().unwrap();
                let crate::service::State { engine, config, .. } = &mut *state;
                engine.action(action, config);
                if action == Action::ToggleAuto {
                    state.config.save(&state.path)?;
                }
                self.undo = None;
                return Ok(());
            }
            Action::SwitchLayout => {
                let layout = shared.lock().unwrap().engine.layout.other();
                return self.set_layout(layout, shared);
            }
            _ => {}
        }
        if let (Some(input), Some(keyboard)) = (&mut self.input, &mut self.keyboard)
            && input.active
        {
            input.action(action, keyboard, shared)?;
            keyboard.reported = Some(shared.lock().unwrap().engine.layout);
            return Ok(());
        }
        let target = self
            .target
            .clone()
            .context("Нет доступного текстового поля")?;
        let before = target.snapshot(&self.connection)?;
        if action == Action::Undo {
            let undo = self.undo.take().context("Нет преобразования для отмены")?;
            ensure!(
                undo.target == target && undo.expected == before,
                "Текст или курсор изменились; откат отменён"
            );
            let (expected, _) = self.replace(
                &target,
                &before,
                undo.start..undo.start + undo.replacement_len,
                &undo.original,
                undo.layout,
            )?;
            self.last_edit = Some((target.clone(), expected));
            return self.finish_replacement(undo.layout, shared);
        }
        let (start, end) = if before.cursor != before.anchor {
            (
                before.cursor.min(before.anchor),
                before.cursor.max(before.anchor),
            )
        } else if action == Action::ConvertWord {
            word_range(&before.text, before.cursor).context("Нет слова для преобразования")?
        } else {
            bail!("Выделите текст в приложении");
        };
        let original: String = before.text.chars().skip(start).take(end - start).collect();
        let replacement = match action {
            Action::ChangeCase => {
                if original
                    .chars()
                    .filter(|c| c.is_alphabetic())
                    .all(char::is_uppercase)
                {
                    original.to_lowercase()
                } else {
                    original.to_uppercase()
                }
            }
            Action::Transliterate => layout::transliterate(&original),
            _ => layout::convert_selection(&original),
        };
        if replacement == original {
            return Ok(());
        }
        let layout = shared.lock().unwrap().engine.layout;
        let final_layout = if matches!(action, Action::ConvertSelection | Action::ConvertWord) {
            word_layout(&replacement, layout)
        } else {
            layout
        };
        let (expected, event_serial) =
            self.replace(&target, &before, start..end, &replacement, final_layout)?;
        self.last_edit = Some((target.clone(), expected.clone()));
        self.undo = Some(Undo {
            event_serial,
            target: target.clone(),
            expected,
            start,
            replacement_len: replacement.chars().count(),
            original,
            layout,
        });
        if matches!(action, Action::ConvertSelection | Action::ConvertWord) {
            self.finish_replacement(final_layout, shared)?;
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.connection.clone().close();
    }
}

fn registry_signature(xml: &str) -> Result<String> {
    let node = zbus_xml::Node::from_reader(xml.as_bytes())?;
    let method = node
        .interfaces()
        .iter()
        .find(|interface| interface.name().as_str() == "org.a11y.atspi.Registry")
        .and_then(|interface| {
            interface
                .methods()
                .iter()
                .find(|method| method.name().as_str() == "RegisterEvent")
        })
        .context("AT-SPI не предоставляет RegisterEvent")?;
    Ok(method
        .args()
        .iter()
        .filter(|arg| arg.direction() != Some(zbus_xml::ArgDirection::Out))
        .map(|arg| arg.ty().to_string())
        .collect())
}
fn serial_after(serial: u32, earlier: u32) -> bool {
    let delta = serial.wrapping_sub(earlier);
    delta != 0 && delta < u32::MAX / 2
}
fn splice(text: &str, start: usize, end: usize, replacement: &str) -> String {
    text.chars()
        .take(start)
        .chain(replacement.chars())
        .chain(text.chars().skip(end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_word_ranges_preserve_context() {
        assert!(serial_after(1, u32::MAX));
        assert!(!serial_after(12, 12));
        assert!(!serial_after(11, 12));
        assert_eq!(
            completed_word("😀 руддщ ", 8),
            Some((2, "руддщ".into(), " ".into()))
        );
        assert_eq!(
            completed_word("user@example.com ", 17).unwrap().1,
            "user@example.com"
        );
        assert_eq!(completed_word("word  ", 6), None);
        assert_eq!(splice("😀 руддщ text", 2, 7, "hello"), "😀 hello text");
        assert_eq!(word_range("hello ", 100), None);
    }
    #[test]
    fn registry_versions_from_introspection() {
        for types in [vec!["s"], vec!["s", "as"], vec!["s", "as", "s"]] {
            let args: String = types
                .iter()
                .map(|ty| format!("<arg type='{ty}' direction='in'/>"))
                .collect();
            let xml = format!(
                "<node><interface name='org.a11y.atspi.Registry'><method name='RegisterEvent'>{args}</method></interface></node>"
            );
            assert_eq!(registry_signature(&xml).unwrap(), types.concat());
        }
        assert!(registry_signature("<node/>").is_err());
    }
}
