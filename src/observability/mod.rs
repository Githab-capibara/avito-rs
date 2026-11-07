//! Наблюдаемость: `tracing`-спаны, счётчики метрик, ротация логов.
//!
//! Порт `logger.py` (функция `get_logger`) + `utils/log_cleanup.py`
//! (`clean_old_logs`: возраст + лимит файлов).

pub mod cleanup;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::dto::ObservabilityCfg;

/// Счётчики процесса (атомики вместо крейта metrics).
#[derive(Debug, Default)]
pub struct Metrics {
    pages: AtomicU64,
    new_ads: AtomicU64,
    fetch_errors: AtomicU64,
    parse_errors: AtomicU64,
    notify_errors: AtomicU64,
    blocks: AtomicU64,
}

impl Metrics {
    /// Пустые метрики.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pages: AtomicU64::new(0),
            new_ads: AtomicU64::new(0),
            fetch_errors: AtomicU64::new(0),
            parse_errors: AtomicU64::new(0),
            notify_errors: AtomicU64::new(0),
            blocks: AtomicU64::new(0),
        }
    }

    fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn load(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }

    #[doc(hidden)]
    pub fn inc_pages(&self) {
        Self::bump(&self.pages);
    }
    #[doc(hidden)]
    pub fn inc_new_ads(&self) {
        Self::bump(&self.new_ads);
    }
    #[doc(hidden)]
    pub fn inc_fetch_errors(&self) {
        Self::bump(&self.fetch_errors);
    }
    #[doc(hidden)]
    pub fn inc_parse_errors(&self) {
        Self::bump(&self.parse_errors);
    }
    #[doc(hidden)]
    pub fn inc_notify_errors(&self) {
        Self::bump(&self.notify_errors);
    }
    #[doc(hidden)]
    pub fn inc_blocks(&self) {
        Self::bump(&self.blocks);
    }

    /// Снимок всех счётчиков.
    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            pages: Self::load(&self.pages),
            new_ads: Self::load(&self.new_ads),
            fetch_errors: Self::load(&self.fetch_errors),
            parse_errors: Self::load(&self.parse_errors),
            notify_errors: Self::load(&self.notify_errors),
            blocks: Self::load(&self.blocks),
        }
    }
}

/// Снимок метрик (для `status`, API, тестов).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct MetricsSnapshot {
    /// Обработано страниц.
    pub pages: u64,
    /// Новых объявлений.
    pub new_ads: u64,
    /// Ошибок сети.
    pub fetch_errors: u64,
    /// Ошибок парсинга.
    pub parse_errors: u64,
    /// Ошибок уведомлений.
    pub notify_errors: u64,
    /// Зафиксировано блокировок.
    pub blocks: u64,
}

/// Инициализировать `tracing-subscriber` из конфига + ротация логов.
pub fn init_logging(cfg: &ObservabilityCfg) -> anyhow::Result<()> {
    std::fs::create_dir_all(&cfg.log_dir).map_err(|e| anyhow::anyhow!("create log dir: {e}"))?;

    tracing_subscriber::fmt()
        .with_max_level(
            cfg.log_level
                .parse::<tracing::Level>()
                .unwrap_or(tracing::Level::INFO),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(true)
        .init();

    cleanup::clean_old_logs(&cfg.log_dir, cfg.log_keep_days, cfg.log_max_files)?;
    Ok(())
}
