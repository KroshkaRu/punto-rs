//! Локальный ярлык Perplexity включает AT-SPI только для приложения; исходный ярлык сохраняется для отката.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const NAME: &str = "perplexity-desktop_perplexity-desktop.desktop";
#[derive(Serialize, Deserialize)]
struct Backup {
    original: Option<String>,
    installed: String,
}
pub fn paths() -> (PathBuf, PathBuf) {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    (
        PathBuf::from("/var/lib/snapd/desktop/applications").join(NAME),
        data.join("applications").join(NAME),
    )
}
fn backup(target: &Path) -> PathBuf {
    target.with_extension("desktop.punto-rs-backup")
}
pub fn installed(target: &Path) -> bool {
    backup(target).exists()
}
fn write(path: &Path, value: &str) -> Result<()> {
    let parent = path.parent().context("Нет каталога ярлыка")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(value.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
pub fn enable(source: &Path, target: &Path) -> Result<()> {
    ensure!(!installed(target), "Поддержка уже включена");
    let original = match fs::read_to_string(target) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let text = match &original {
        Some(s) => s.clone(),
        None => fs::read_to_string(source)?,
    };
    let mut found = false;
    let changed = text
        .lines()
        .map(|line| {
            if let Some(exec) = line.strip_prefix("Exec=") {
                found = true;
                format!("Exec=env ACCESSIBILITY_ENABLED=1 {exec} --force-renderer-accessibility")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    ensure!(found, "Ярлык не содержит Exec");
    write(
        &backup(target),
        &toml::to_string(&Backup {
            original,
            installed: changed.clone(),
        })?,
    )?;
    write(target, &changed)
}
pub fn restore(target: &Path) -> Result<()> {
    let path = backup(target);
    let saved: Backup = toml::from_str(&fs::read_to_string(&path)?)?;
    let current = match fs::read_to_string(target) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if current == saved.original {
        fs::remove_file(path)?;
        return Ok(());
    }
    ensure!(
        current.as_deref() == Some(saved.installed.as_str()),
        "Ярлык изменён после установки; резервная копия сохранена"
    );
    if let Some(original) = saved.original {
        write(target, &original)?;
    } else {
        fs::remove_file(target)?;
    }
    fs::remove_file(path)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_override_and_restore_preserve_user_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("system.desktop");
        let target = dir.path().join("local.desktop");
        fs::write(
            &source,
            "[Desktop Entry]\nExec=/snap/bin/perplexity-desktop\nName=Perplexity\n",
        )
        .unwrap();
        for original in [
            None,
            Some("[Desktop Entry]\nExec=/snap/bin/perplexity-desktop %U\nName=Custom\n"),
        ] {
            if let Some(s) = original {
                fs::write(&target, s).unwrap();
            }
            enable(&source, &target).unwrap();
            let changed = fs::read_to_string(&target).unwrap();
            assert!(changed.contains("Exec=env ACCESSIBILITY_ENABLED=1 "));
            assert!(changed.contains("--force-renderer-accessibility"));
            assert!(enable(&source, &target).is_err());
            fs::write(&target, "user edit").unwrap();
            assert!(restore(&target).is_err());
            fs::write(&target, changed).unwrap();
            restore(&target).unwrap();
            assert_eq!(fs::read_to_string(&target).ok().as_deref(), original);
        }
    }
}
