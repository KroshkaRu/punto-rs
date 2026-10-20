//! Поиск системных сочетаний GSettings; явное освобождение с сохранением и восстановлением исходных значений.
use crate::hotkey::Hotkey;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path, process::Command};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemBinding {
    pub schema: String,
    pub key: String,
    pub value: String,
}
impl SystemBinding {
    pub fn matches(&self, key: Hotkey) -> bool {
        quoted_strings(&self.value)
            .iter()
            .any(|value| accelerator(value) == Some(key))
    }
    fn disabled(&self) -> &str {
        if self.value.starts_with('[') || self.value.starts_with("@as") {
            "@as []"
        } else {
            "''"
        }
    }
}
pub fn scan() -> Vec<SystemBinding> {
    let mut bindings = vec![];
    for schema in [
        "org.mate.Marco.global-keybindings",
        "org.mate.Marco.window-keybindings",
        "org.mate.SettingsDaemon.plugins.media-keys",
        "org.gnome.desktop.wm.keybindings",
        "org.gnome.settings-daemon.plugins.media-keys",
        "org.gnome.shell.keybindings",
    ] {
        let Ok(output) = Command::new("gsettings")
            .args(["list-recursively", schema])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut parts = line.splitn(3, ' ');
            if let (Some(schema), Some(key), Some(value)) =
                (parts.next(), parts.next(), parts.next())
                && quoted_strings(value)
                    .iter()
                    .any(|s| accelerator(s).is_some())
            {
                bindings.push(SystemBinding {
                    schema: schema.into(),
                    key: key.into(),
                    value: value.trim().into(),
                });
            }
        }
    }
    bindings
}
fn get(entry: &SystemBinding) -> Result<String> {
    let output = Command::new("gsettings")
        .args(["get", &entry.schema, &entry.key])
        .output()?;
    ensure!(
        output.status.success(),
        "Не удалось прочитать {} / {}",
        entry.schema,
        entry.key
    );
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
fn set(entry: &SystemBinding, value: &str) -> Result<()> {
    let output = Command::new("gsettings")
        .args(["set", &entry.schema, &entry.key, value])
        .output()?;
    ensure!(
        output.status.success(),
        "{} / {}: {}",
        entry.schema,
        entry.key,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Backup {
    entries: Vec<SystemBinding>,
}
fn save_backup(path: &Path, entries: Vec<SystemBinding>) -> Result<()> {
    let parent = path
        .parent()
        .context("Нет каталога резервной копии сочетаний")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(toml::to_string(&Backup { entries })?.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
pub fn release(path: &Path, entries: &[SystemBinding]) -> Result<()> {
    ensure!(
        !path.exists(),
        "Сначала восстановите ранее отключённые системные сочетания"
    );
    ensure!(!entries.is_empty(), "Известных системных конфликтов нет");
    for entry in entries {
        ensure!(
            get(entry)? == entry.value,
            "Системное сочетание изменилось; повторите проверку"
        );
    }
    // Persist the originals before changing even the first desktop binding.
    save_backup(path, entries.to_vec())?;
    for entry in entries {
        if let Err(error) = set(entry, entry.disabled()) {
            return match restore(path) {
                Ok(()) => Err(error),
                Err(rollback) => {
                    Err(error.context(format!("Восстановление не завершено: {rollback:#}")))
                }
            };
        }
    }
    Ok(())
}
pub fn restore(path: &Path) -> Result<()> {
    let backup: Backup = toml::from_str(&fs::read_to_string(path)?)?;
    let mut remaining = vec![];
    for entry in backup.entries {
        let restored = (|| -> Result<()> {
            let current = get(&entry)?;
            if current == entry.value {
                return Ok(());
            }
            ensure!(
                current == entry.disabled(),
                "Системное сочетание изменено другим приложением"
            );
            set(&entry, &entry.value)
        })();
        if restored.is_err() {
            remaining.push(entry);
        }
    }
    if remaining.is_empty() {
        fs::remove_file(path)?;
    } else {
        let count = remaining.len();
        save_backup(path, remaining)?;
        bail!(
            "Не восстановлено сочетаний: {count}. Резервная копия сохранена; проверьте изменения в настройках системы"
        );
    }
    Ok(())
}
fn accelerator(text: &str) -> Option<Hotkey> {
    let mut rest = text;
    let mut parts = vec![];
    while rest.starts_with('<') {
        let end = rest.find('>')?;
        parts.push(match &rest[1..end].to_ascii_lowercase()[..] {
            "control" | "primary" | "ctrl" => "Ctrl",
            "alt" | "mod1" => "Alt",
            "shift" => "Shift",
            "super" | "mod4" => "Super",
            _ => return None,
        });
        rest = &rest[end + 1..];
    }
    parts.push(if rest == "Scroll_Lock" {
        "ScrollLock"
    } else {
        rest
    });
    Hotkey::parse(&parts.join("+")).ok()
}
fn quoted_strings(text: &str) -> Vec<String> {
    let mut result = vec![];
    let mut quote = None;
    let mut current = String::new();
    let mut escape = false;
    for ch in text.chars() {
        if let Some(delimiter) = quote {
            if escape {
                current.push(ch);
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == delimiter {
                result.push(std::mem::take(&mut current));
                quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gtk_accelerators_and_variant_arrays() {
        for value in ["'<Primary><Alt>t'", "['<Control><Alt>t', '<Super>F9']"] {
            let entry = SystemBinding {
                schema: "test".into(),
                key: "test".into(),
                value: value.into(),
            };
            assert!(entry.matches(Hotkey::parse("Ctrl+Alt+T").unwrap()));
        }
        assert_eq!(accelerator("<Mod4>F9"), Hotkey::parse("Super+F9").ok());
        assert_eq!(accelerator("disabled"), None);
        assert_eq!(quoted_strings("@as []"), Vec::<String>::new());
    }
    #[test]
    #[ignore = "Requires an isolated D-Bus session with MATE schemas"]
    fn release_and_restore_real_gsettings() {
        assert_eq!(std::env::var("PUNTO_TEST_SHORTCUTS").as_deref(), Ok("1"));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backup.toml");
        let mut entries = [
            ("org.mate.Marco.global-keybindings", "run-command-terminal"),
            ("org.gnome.desktop.wm.keybindings", "close"),
        ]
        .into_iter()
        .map(|(schema, key)| SystemBinding {
            schema: schema.into(),
            key: key.into(),
            value: String::new(),
        })
        .collect::<Vec<_>>();
        for entry in &mut entries {
            entry.value = get(entry).unwrap();
        }
        release(&path, &entries).unwrap();
        for entry in &entries {
            assert_eq!(get(entry).unwrap(), entry.disabled());
        }
        restore(&path).unwrap();
        for entry in &entries {
            assert_eq!(get(entry).unwrap(), entry.value);
        }
        assert!(!path.exists());
    }
}
