//! Ротация логов: порт `utils/log_cleanup.py`.
//!
//! Удаляет лог-файлы старше `max_age_days`, затем режет оставшиеся до
//! `max_files` самых свежих. Ошибки ФС не роняют процесс.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Признак «это лог-файл»: имя содержит `.log` или суффикс архив.
fn is_log_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.contains(".log") || name.ends_with(".zip") || name.ends_with(".gz")
}

/// Возраст файла, секунды.
fn age_secs(path: &Path) -> Option<u64> {
    let md = std::fs::metadata(path).ok()?;
    let modified = md.modified().ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .ok()?;
    Some(age.as_secs())
}

/// Очистить логи: сначала по возрасту, потом по количеству.
pub fn clean_old_logs(
    log_dir: &Path,
    max_age_days: u64,
    max_files: u64,
) -> anyhow::Result<Vec<PathBuf>> {
    if !log_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut removed = Vec::new();
    let mut remaining: Vec<PathBuf> = Vec::new();

    let entries = std::fs::read_dir(log_dir)
        .map_err(|e| anyhow::anyhow!("read log dir {}: {e}", log_dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_log_file(&path) {
            continue;
        }
        match age_secs(&path) {
            Some(age) if age > max_age_days.saturating_mul(86_400) => {
                if std::fs::remove_file(&path).is_ok() {
                    removed.push(path);
                }
            }
            _ => remaining.push(path),
        }
    }

    if max_files > 0 && remaining.len() > max_files as usize {
        remaining.sort_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH)
        });
        let excess = remaining.len() - max_files as usize;
        for path in remaining.into_iter().take(excess) {
            if std::fs::remove_file(&path).is_ok() {
                removed.push(path);
            }
        }
    }

    Ok(removed)
}

/// JSON-лог: безопасная запись события (секреты уже отредактированы выше).
pub fn append_json_line(path: &Path, line: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
    f.write_all(line.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
    Ok(())
}

/// Сколько секунд ждать между чистильщиками (1 час).
#[must_use]
pub const fn cleanup_interval() -> Duration {
    Duration::from_secs(3_600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_by_age_and_count() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            let p = dir.path().join(format!("run-{i}.log"));
            std::fs::write(&p, b"x").unwrap();
        }
        let removed = clean_old_logs(dir.path(), 0, 2).unwrap();
        assert_eq!(removed.len(), 3, "keep only 2 newest");
        let left = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|d| d.path().extension().map(|x| x == "log"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(left, 2);
    }

    #[test]
    fn ignores_non_log_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.json"), b"{}").unwrap();
        let removed = clean_old_logs(dir.path(), 0, 1).unwrap();
        assert!(removed.is_empty());
        assert!(dir.path().join("data.json").exists());
    }

    #[test]
    fn missing_dir_is_ok() {
        let removed = clean_old_logs(Path::new("/nonexistent-dir-xyz"), 5, 10);
        assert!(removed.is_ok());
    }
}
