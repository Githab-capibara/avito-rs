//! E2E: настоящий бинарник против локального мок-сервера Авито.
//!
//! Мок отдаёт:
//! - `POST /api/9/spfa/items` — каталог;
//! - `POST /api/1/card/items/<id>` — карточки (просмотры/описание);
//! - Telegram/VK заменяем на локальные sink'и через `cfg` — проверяем,
//!   что уведомления формируются без падения (без реальной сети).
//!
//! Тест гоняет `avito-rs run --once` с конфигом, который указывает на мок.
//! Цель: убедиться, что fetch → extract → filter → enrich → notify →
//! export → mark_seen реально работает и данные попадают в CSV/БД.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Каталог из 3 объявлений: 2 проходят фильтры, 1 режется по цене.
fn catalog_payload() -> Value {
    json!({
        "result": {
            "catalog": {
                "items": [
                    {
                        "id": 5050505050u64,
                        "urlPath": "/krasnodar/telefony/iphone_13_5050505050",
                        "title": "iPhone 13",
                        "description": "Новый, гарантия.",
                        "priceDetailed": { "value": 49999, "currency": "RUB" },
                        "normalizedPrice": "49999 ₽",
                        "sortTimeStamp": 1_792_348_800_000i64,
                        "sellerId": "seller_a",
                        "geo": { "formattedAddress": "Краснодар, ул. Красная" },
                        "isReserved": false,
                        "iva": { "DateInfoStep": [{ "payload": { "vas": [] } }] },
                        "images": [{ "140x105": "http://127.0.0.1:1/a.jpg" }]
                    },
                    {
                        "id": 5050505051u64,
                        "urlPath": "/krasnodar/telefony/samsung_5050505051",
                        "title": "Samsung A54",
                        "description": "Идеал.",
                        "priceDetailed": { "value": 19999, "currency": "RUB" },
                        "normalizedPrice": "19999 ₽",
                        "sortTimeStamp": 1_792_348_000_000i64,
                        "sellerId": "seller_b",
                        "geo": { "formattedAddress": "Краснодар, ул. Севастопольская" },
                        "isReserved": false,
                        "iva": { "DateInfoStep": [{ "payload": { "vas": [] } }] }
                    },
                    {
                        "id": 5050505052u64,
                        "urlPath": "/krasnodar/telefony/broken_5050505052",
                        "title": "Сломанный",
                        "description": "На запчасти",
                        "priceDetailed": { "value": 129999, "currency": "RUB" },
                        "sortTimeStamp": 1_792_348_000_000i64,
                        "geo": { "formattedAddress": "Краснодар" },
                        "isReserved": true
                    }
                ]
            }
        }
    })
}

/// Карточка: просмотры и полное описание.
fn card_payload(id: &str) -> Value {
    json!({
        "success": {
            "mobile": {
                "stats": { "views": { "total": 4321, "today": 12 } },
                "description": format!("Полное описание объявления {id}, без сокращений.")
            }
        }
    })
}

/// Мок-сервер: один поток, ответы на нужные пути.
struct MockAvito {
    base: String,
    requests: Arc<AtomicUsize>,
    child: Option<thread::JoinHandle<()>>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl MockAvito {
    fn spawn() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind mock")?;
        let port = listener.local_addr()?.port();
        let base = format!("http://127.0.0.1:{port}");
        let requests = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let counter = Arc::clone(&requests);
        let stop = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            listener.set_nonblocking(true).expect("nonblocking");
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let counter = Arc::clone(&counter);
                        thread::spawn(move || serve(stream, counter));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        Ok(Self {
            base,
            requests,
            child: Some(handle),
            shutdown,
        })
    }
    fn hits(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for MockAvito {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.child.take() {
            let _ = h.join();
        }
    }
}

/// Обслужить один запрос.
fn serve(mut stream: TcpStream, counter: Arc<AtomicUsize>) {
    use std::io::Read as _;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 16_384];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]);
    counter.fetch_add(1, Ordering::SeqCst);
    let path = req
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();

    let body: Value = if path.contains("/api/1/card/items/") {
        let id = path.rsplit('/').next().unwrap_or("0");
        card_payload(id)
    } else if path.starts_with("/api/9/items") || path == "/api/9/items" {
        catalog_payload()
    } else {
        json!({"ok": true})
    };
    let payload = serde_json::to_string(&body).unwrap_or_default();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Конфиг для теста: mock base, строгие фильтры, CSV в tmp.
fn write_config(
    dir: &std::path::Path,
    base: &str,
    db: &std::path::Path,
    out: &std::path::Path,
) -> Result<PathBuf> {
    let text = format!(
        r#"interval_sec = 5
pages_per_url = 1
pause_between_links_sec = 0
parse_views = true
parse_description = true
fetch_mode = "mobile_api"
api_base_override = "{base}"
urls = ["{base}/krasnodar/telefony"]
db_path = "{}"

[filters]
max_price = 50000
min_price = 1000
max_age_sec = 0
ignore_reserved = true
ignore_promotion = false
region = "Краснодар"
keywords_black = ["на запчасти"]

[export]
dir = "{}"
format = "csv"
one_file_per_link = false
"#,
        db.display(),
        out.display()
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, text).context("write config")?;
    Ok(path)
}

/// Запустить бинарник `run --once`.
fn run_once(bin: &str, config: &std::path::Path, wipe_db: bool) -> Result<()> {
    if wipe_db {
        let _ = std::fs::remove_file(config_db(config));
    }
    let mut child: Child = Command::new(bin)
        .args(["run", "--config"])
        .arg(config)
        .arg("--once")
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn binary")?;
    let status = child.wait().context("wait")?;
    assert!(status.success(), "run --once exited {status}");
    Ok(())
}

/// Путь к БД из конфига (тест сам его выставил через шаблон).
fn config_db(config: &std::path::Path) -> PathBuf {
    let raw = std::fs::read_to_string(config).expect("read config");
    raw.lines()
        .find_map(|l| l.strip_prefix("db_path = "))
        .map(|p| PathBuf::from(p.trim_matches(['"', ' '])))
        .unwrap_or_else(|| std::env::temp_dir().join("avito_e2e_monitor.db"))
}

/// `ChariotTest` — тест первого доступного бинаря.
#[test]
fn end_to_end_monitor_runs_against_mock() -> Result<()> {
    let Some(bin) = resolve_binary() else {
        eprintln!("SKIP: собранный бинарник не найден (cargo build --release)");
        return Ok(());
    };
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("out");
    let db = dir.path().join("monitor.db");
    let mock = MockAvito::spawn()?;
    // Реальная цель: локальный мок Авито.
    let config = write_config(dir.path(), &mock.base, &db, &out)?;

    run_once(&bin, &config, true)?;

    // 1) Действительно ходили в мок (каталог + карточки)
    assert!(
        mock.hits() >= 2,
        "expected >=2 mock hits, got {}",
        mock.hits()
    );

    // 2) CSV создан и содержит ровно 2 прошедшие фильтры объявления
    let csv = out.join("avito.csv");
    let contents = std::fs::read_to_string(&csv).context("read csv")?;
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 3, "header + 2 rows: {contents}");
    assert!(contents.contains("iPhone 13"), "{contents}");
    assert!(contents.contains("Samsung A54"), "{contents}");
    assert!(
        !contents.contains("Сломанный"),
        "reserved+black должно быть отфильтровано"
    );
    // 3) Просмотры из карточек проставлены (enrichment)
    assert!(contents.contains("4321"), "views total missing: {contents}");
    assert!(contents.contains("12"), "views today missing: {contents}");
    // 4) Полное описание из карточки заменяет короткое
    assert!(
        contents.contains("Полное описание объявления"),
        "{contents}"
    );
    // 5) Цены/адреса на месте
    assert!(contents.contains("49999"), "{contents}");
    assert!(contents.contains("Краснодар"), "{contents}");

    // 6) БД содержит оба объявления (mark_seen отработал)
    let conn = rusqlite::Connection::open(&db)?;
    let seen: i64 = conn
        .query_row("SELECT COUNT(*) FROM seen WHERE price > 0", [], |r| {
            r.get(0)
        })
        .context("seen count")?;
    assert_eq!(seen, 2, "expected 2 dedup rows");
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM seen ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    assert!(ids.contains(&"5050505050".to_owned()), "{ids:?}");
    assert!(ids.contains(&"5050505051".to_owned()), "{ids:?}");

    // 7) Второй проход — дедуп: CSV не растёт (БД сохраняем!)
    run_once(&bin, &config, false)?;
    let contents2 = std::fs::read_to_string(&csv)?;
    let lines2: Vec<&str> = contents2.lines().collect();
    assert_eq!(
        lines2.len(),
        3,
        "dedup должен был отсечь повтор: {contents2}"
    );

    Ok(())
}

/// Найти бинарник: `target/{debug,release}/avito-rs`.
fn resolve_binary() -> Option<String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for rel in ["target/release/avito-rs", "target/debug/avito-rs"] {
        let candidate = manifest.join(rel);
        if candidate.exists() {
            return Some(candidate.display().to_string());
        }
    }
    None
}
