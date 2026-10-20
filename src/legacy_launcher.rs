//! Восстановление ярлыков, изменённых версией 0.3.0; новые обходы для приложений не создаются.
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
pub fn path() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join("applications").join(NAME)
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
        let target = dir.path().join("local.desktop");
        for original in [
            None,
            Some("[Desktop Entry]\nExec=/snap/bin/perplexity-desktop %U\nName=Custom\n"),
        ] {
            let changed = "[Desktop Entry]\nExec=env ACCESSIBILITY_ENABLED=1 app --force-renderer-accessibility\n";
            fs::write(&target, changed).unwrap();
            fs::write(
                backup(&target),
                toml::to_string(&Backup {
                    original: original.map(str::to_owned),
                    installed: changed.into(),
                })
                .unwrap(),
            )
            .unwrap();
            fs::write(&target, "user edit").unwrap();
            assert!(restore(&target).is_err());
            fs::write(&target, changed).unwrap();
            restore(&target).unwrap();
            assert_eq!(fs::read_to_string(&target).ok().as_deref(), original);
        }
    }
}
