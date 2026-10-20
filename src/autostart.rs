//! Автозапуск только по выбору пользователя через XDG desktop entry; запускается фоновый процесс.

use anyhow::{Context, Result};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn path() -> PathBuf {
    crate::config::config_home().join("autostart/punto-rs.desktop")
}

pub fn enabled(path: &Path) -> Result<bool> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Не удалось прочитать настройку автозапуска"),
    };
    let mut desktop_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            desktop_entry = line == "[Desktop Entry]";
        } else if desktop_entry
            && let Some((key, value)) = line.split_once('=')
            && matches!(
                (key.trim(), value.trim()),
                ("Hidden", "true") | ("X-GNOME-Autostart-enabled", "false")
            )
        {
            return Ok(false);
        }
    }
    Ok(text.lines().any(|line| line.trim() == "[Desktop Entry]"))
}

pub fn set_enabled(path: &Path, enabled: bool) -> Result<()> {
    if enabled {
        let parent = path.parent().context("Нет каталога автозапуска")?;
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(b"[Desktop Entry]\nType=Application\nName=Punto RS\nExec=punto-rs --daemon\nTryExec=punto-rs\nIcon=punto-rs\nTerminal=false\n")?;
        temp.as_file().sync_all()?;
        temp.persist(path)
            .context("Не удалось включить автозапуск")?;
    } else if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error).context("Не удалось отключить автозапуск");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_desktop_disabled_flags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autostart/punto-rs.desktop");
        assert!(!enabled(&path).unwrap());
        set_enabled(&path, true).unwrap();
        assert!(enabled(&path).unwrap());
        let entry = fs::read_to_string(&path).unwrap();
        assert!(entry.contains("Exec=punto-rs --daemon\n"));
        for flag in ["Hidden=true", "X-GNOME-Autostart-enabled=false"] {
            fs::write(&path, format!("{entry}{flag}\n")).unwrap();
            assert!(!enabled(&path).unwrap());
            set_enabled(&path, true).unwrap();
            assert!(enabled(&path).unwrap());
        }
        fs::write(dir.path().join("autostart/other.desktop"), "keep").unwrap();
        set_enabled(&path, false).unwrap();
        set_enabled(&path, false).unwrap();
        assert!(!enabled(&path).unwrap());
        assert!(dir.path().join("autostart/other.desktop").exists());
    }
}
