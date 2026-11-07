//! Хранилище: SQLite (`rusqlite`) — память просмотренных объявлений и
//! история цен. Порт `database/db_service.py` оригинала.
//!
//! Отличия от оригинала (сознательные, задокументированы в ADR-02):
//! 1. В оригинале в таблице `viewed` нет UNIQUE → накапливались дубликаты
//!    `(id, price)`. Здесь UNIQUE-индекс + `INSERT OR IGNORE`.
//! 2. Дедуп по паре `(id, price)` воспроизводит `DbHandler.is_viewed(id, price)`
//!    оригинала: именно это даёт уведомление при снижении цены.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::dto::Listing;

/// Схема и миграции БД.
const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_init",
    "CREATE TABLE IF NOT EXISTS seen (
        id          TEXT NOT NULL,
        price       INTEGER NOT NULL DEFAULT 0,
        url         TEXT NOT NULL DEFAULT '',
        first_seen  INTEGER NOT NULL,
        UNIQUE(id, price)
    );
    CREATE INDEX IF NOT EXISTS idx_seen_id ON seen(id);
    CREATE TABLE IF NOT EXISTS prices (
        id          TEXT NOT NULL,
        price       INTEGER NOT NULL,
        changed_at  INTEGER NOT NULL,
        PRIMARY KEY (id)
    );
    CREATE TABLE IF NOT EXISTS runs (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        started_at  INTEGER NOT NULL,
        finished_at INTEGER,
        pages       INTEGER NOT NULL DEFAULT 0,
        new_ads     INTEGER NOT NULL DEFAULT 0,
        errors      INTEGER NOT NULL DEFAULT 0
    );",
)];

/// SQLite-хранилище.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Открыть/создать БД и применить миграции.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create db dir {}", parent.display()))?;
            }
        }
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite {}", path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .context("set pragmas")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Применить миграции (идемпотентно).
    fn migrate(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                name TEXT PRIMARY KEY NOT NULL,
                applied_at INTEGER NOT NULL
            );",
        )
        .context("create _migrations")?;
        for (name, sql) in MIGRATIONS {
            let already: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM _migrations WHERE name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if already > 0 {
                continue;
            }
            conn.execute_batch(sql)
                .with_context(|| format!("apply migration {name}"))?;
            conn.execute(
                "INSERT INTO _migrations (name, applied_at) VALUES (?1, ?2)",
                rusqlite::params![name, now_ts()],
            )
            .with_context(|| format!("record migration {name}"))?;
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Проверить, видели ли объявление именно по этой цене.
    pub fn is_seen(&self, listing: &Listing) -> Result<bool> {
        let price = listing.price.unwrap_or(0).to_string();
        let n: i64 = self
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM seen WHERE id = ?1 AND price = ?2",
                rusqlite::params![listing.id, price],
                |r| r.get(0),
            )
            .context("query seen")?;
        Ok(n > 0)
    }

    /// Пометить объявление просмотренным.
    pub fn mark_seen(&self, listing: &Listing) -> Result<()> {
        let price = listing.price.unwrap_or(0);
        self.lock()
            .execute(
                "INSERT OR IGNORE INTO seen (id, price, url, first_seen) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![listing.id, price, listing.url(), now_ts()],
            )
            .context("insert seen")?;
        Ok(())
    }

    /// Последняя известная цена объявления (для трекинга снижений).
    pub fn last_price(&self, id: &str) -> Result<Option<u64>> {
        let price: Option<i64> = self
            .lock()
            .query_row("SELECT price FROM prices WHERE id = ?1", [id], |r| r.get(0))
            .ok();
        Ok(price.and_then(|p| u64::try_from(p).ok()))
    }

    /// Записать текущую цену (upsert).
    pub fn set_price(&self, id: &str, price: u64) -> Result<()> {
        self.lock()
            .execute(
                "INSERT INTO prices (id, price, changed_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET price = excluded.price,
                                             changed_at = excluded.changed_at",
                rusqlite::params![id, i64::try_from(price).unwrap_or(0), now_ts()],
            )
            .context("upsert price")?;
        Ok(())
    }

    /// Начать запись о запуске цикла → id.
    pub fn start_run(&self) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO runs (started_at) VALUES (?1)",
            rusqlite::params![now_ts()],
        )
        .context("start run")?;
        Ok(conn.last_insert_rowid())
    }

    /// Завершить запуск цикла.
    pub fn finish_run(&self, run_id: i64, pages: u64, new_ads: u64, errors: u64) -> Result<()> {
        self.lock()
            .execute(
                "UPDATE runs SET finished_at = ?1, pages = ?2, new_ads = ?3, errors = ?4 WHERE id = ?5",
                rusqlite::params![
                    now_ts(),
                    i64::try_from(pages).unwrap_or(0),
                    i64::try_from(new_ads).unwrap_or(0),
                    i64::try_from(errors).unwrap_or(0),
                    run_id
                ],
            )
            .context("finish run")?;
        Ok(())
    }

    /// Статистика по просмотренным (для `status`/тестов).
    pub fn seen_count(&self) -> Result<u64> {
        let n: i64 = self
            .lock()
            .query_row("SELECT COUNT(*) FROM seen", [], |r| r.get(0))
            .context("count seen")?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}

/// Unix-время в секундах.
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

/// Трейт-порт хранилища (для интеграционных тестов с моком).
pub trait ListingStore: Send + Sync {
    /// Видели ли объявление по этой цене.
    fn is_seen(&self, listing: &Listing) -> Result<bool>;
    /// Пометить просмотренным.
    fn mark_seen(&self, listing: &Listing) -> Result<()>;
    /// Последняя цена.
    fn last_price(&self, id: &str) -> Result<Option<u64>>;
    /// Записать цену.
    fn set_price(&self, id: &str, price: u64) -> Result<()>;
}

impl ListingStore for SqliteStore {
    fn is_seen(&self, listing: &Listing) -> Result<bool> {
        Self::is_seen(self, listing)
    }
    fn mark_seen(&self, listing: &Listing) -> Result<()> {
        Self::mark_seen(self, listing)
    }
    fn last_price(&self, id: &str) -> Result<Option<u64>> {
        Self::last_price(self, id)
    }
    fn set_price(&self, id: &str, price: u64) -> Result<()> {
        Self::set_price(self, id, price)
    }
}

impl SqliteStore {
    /// In-memory БД (для интеграционных тестов и `--once` без диска).
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory sqlite")?;
        conn.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA foreign_keys=ON;")
            .context("set pragmas")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(id: &str, price: Option<u64>) -> Listing {
        Listing {
            id: id.to_owned(),
            price,
            ..Listing::default()
        }
    }

    #[test]
    fn dedup_by_id_and_price() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("t.db")).unwrap();
        let a = listing("1", Some(100));
        assert!(!store.is_seen(&a).unwrap());
        store.mark_seen(&a).unwrap();
        assert!(store.is_seen(&a).unwrap());

        let cheaper = listing("1", Some(90));
        assert!(!store.is_seen(&cheaper).unwrap(), "price drop must be new");
        store.mark_seen(&cheaper).unwrap();
        assert_eq!(store.seen_count().unwrap(), 2, "unique (id,price) rows");

        store.mark_seen(&a).unwrap();
        assert_eq!(store.seen_count().unwrap(), 2, "mark_seen is idempotent");
    }

    #[test]
    fn price_tracking_upserts() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("t.db")).unwrap();
        assert_eq!(store.last_price("7").unwrap(), None);
        store.set_price("7", 500).unwrap();
        assert_eq!(store.last_price("7").unwrap(), Some(500));
        store.set_price("7", 450).unwrap();
        assert_eq!(store.last_price("7").unwrap(), Some(450));
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        {
            let _s = SqliteStore::open(&p).unwrap();
        }
        let s = SqliteStore::open(&p).unwrap();
        assert!(s.seen_count().is_ok());
    }
}
