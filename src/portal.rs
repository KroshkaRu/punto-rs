//! Разрешённые пользователем горячие клавиши Wayland через GlobalShortcuts portal.

use crate::{
    config::Config,
    hotkey::{self, Hotkey},
    service::Shared,
};
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zbus::{
    blocking::{Connection, MessageIterator, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

type Dict = HashMap<String, OwnedValue>;
fn value(text: &str) -> OwnedValue {
    Value::from(text).try_to_owned().expect("owned string")
}

pub struct Portal {
    stop: Arc<AtomicBool>,
    connection: Arc<Mutex<Option<Connection>>>,
}
impl Portal {
    pub fn start(shared: Shared) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let connection = Arc::new(Mutex::new(None));
        let worker_stop = stop.clone();
        let worker_connection = connection.clone();
        std::thread::spawn(move || {
            if let Err(error) = run(&shared, &worker_stop, &worker_connection)
                && !worker_stop.load(Ordering::Relaxed)
            {
                shared.lock().unwrap().message = format!("Горячие клавиши Wayland: {error:#}");
            }
        });
        Self { stop, connection }
    }
}
impl Drop for Portal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(connection) = self.connection.lock().unwrap().take() {
            let _ = connection.close();
        }
    }
}

fn response(iterator: &mut MessageIterator, path: &OwnedObjectPath) -> Result<Dict> {
    for message in iterator {
        let message = message?;
        if message
            .header()
            .path()
            .is_some_and(|p| p.as_str() == path.as_str())
        {
            let (code, results): (u32, Dict) = message.body().deserialize()?;
            ensure!(code == 0, "Разрешение отклонено или диалог закрыт");
            return Ok(results);
        }
    }
    anyhow::bail!("Соединение с порталом закрыто")
}
fn run(shared: &Shared, stop: &AtomicBool, slot: &Mutex<Option<Connection>>) -> Result<()> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_secs(5))
        .build()?;
    *slot.lock().unwrap() = Some(connection.clone());
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    let portal = Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.GlobalShortcuts",
    )?;
    let _: u32 = portal.get_property("version")?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.portal.Desktop")?
        .interface("org.freedesktop.portal.Request")?
        .member("Response")?
        .build();
    let mut responses = MessageIterator::for_match_rule(rule, &connection, Some(16))?;
    let options = Dict::from([
        ("handle_token".into(), value("punto_create")),
        ("session_handle_token".into(), value("punto_shortcuts")),
    ]);
    let request: OwnedObjectPath = portal.call("CreateSession", &(options,))?;
    let mut result = response(&mut responses, &request)?;
    let session: String = result
        .remove("session_handle")
        .ok_or_else(|| anyhow::anyhow!("Портал не вернул сеанс"))?
        .try_into()?;
    let session: OwnedObjectPath = session.try_into()?;
    let config = shared.lock().unwrap().config.clone();
    let shortcuts = shortcuts(&config)?;
    let request: OwnedObjectPath = portal.call(
        "BindShortcuts",
        &(
            &session,
            shortcuts,
            "",
            Dict::from([("handle_token".into(), value("punto_bind"))]),
        ),
    )?;
    let bound = response(&mut responses, &request)?;
    ensure!(
        bound.contains_key("shortcuts"),
        "Портал не подтвердил сочетания"
    );
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.portal.Desktop")?
        .interface("org.freedesktop.portal.GlobalShortcuts")?
        .member("Activated")?
        .build();
    let iterator = MessageIterator::for_match_rule(rule, &connection, Some(32))?;
    shared.lock().unwrap().message =
        "Глобальные сочетания подключены через портал рабочего стола".into();
    for message in iterator {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let (event_session, id, _, _): (OwnedObjectPath, String, u64, Dict) =
            message?.body().deserialize()?;
        if event_session != session {
            continue;
        }
        if let Some(action) = crate::config::Action::from_id(&id) {
            let mut state = shared.lock().unwrap();
            if state.actions.len() < 16 {
                state.actions.push_back(action);
            }
        }
    }
    Ok(())
}
fn shortcuts(config: &Config) -> Result<Vec<(String, Dict)>> {
    config
        .hotkeys
        .iter()
        .map(|binding| {
            Ok((
                binding.action.id().into(),
                Dict::from([
                    ("description".into(), value(binding.action.label())),
                    ("preferred_trigger".into(), value(&trigger(&binding.keys)?)),
                ]),
            ))
        })
        .collect()
}
fn trigger(text: &str) -> Result<String> {
    let key = Hotkey::parse(text)?;
    let mut result = String::new();
    for (mask, name) in [
        (hotkey::CONTROL, "CTRL+"),
        (hotkey::ALT, "ALT+"),
        (hotkey::SHIFT, "SHIFT+"),
        (hotkey::SUPER, "LOGO+"),
    ] {
        if key.modifiers & mask != 0 {
            result.push_str(name);
        }
    }
    result.push_str(&match key.key {
        32 => "space".into(),
        0xff13 => "Pause".into(),
        0xff14 => "Scroll_Lock".into(),
        0xff08 => "BackSpace".into(),
        0xff09 => "Tab".into(),
        0xff0d => "Return".into(),
        0xff1b => "Escape".into(),
        0xffbe..=0xffd5 => format!("F{}", key.key - 0xffbd),
        _ => char::from_u32(key.key).unwrap().to_string(),
    });
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xdg_triggers_and_stable_action_ids() {
        assert_eq!(trigger("Ctrl+Alt+S").unwrap(), "CTRL+ALT+s");
        assert_eq!(trigger("ScrollLock").unwrap(), "Scroll_Lock");
        assert_eq!(trigger("Shift+F12").unwrap(), "SHIFT+F12");
        assert_eq!(shortcuts(&Config::default()).unwrap().len(), 8);
        for binding in Config::default().hotkeys {
            assert_eq!(
                crate::config::Action::from_id(binding.action.id()),
                Some(binding.action)
            );
        }
    }
}
