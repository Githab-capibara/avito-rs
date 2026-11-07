//! `avito-rs` — Rust-порт `parser_avito`: монитор объявлений Авито с
//! фильтрами, дедупликацией по цене, уведомлениями в Telegram/VK,
//! экспортом в CSV/XLSX и опциональным REST API.
//!
//! Команды: `run`, `status`, `validate-config`.

use std::sync::Arc;

use anyhow::{Context, Result};
#[cfg(feature = "api")]
use avito_rs::api;
use avito_rs::application::Monitor;
use avito_rs::dto::AppConfig;
use avito_rs::exporters;
use avito_rs::extractors;
use avito_rs::observability::{self, Metrics};
use avito_rs::providers::{self, avito::AvitoClient};
use avito_rs::security::Redactor;
use avito_rs::storage::SqliteStore;
use clap::{Parser, Subcommand};

/// CLI.
#[derive(Debug, Parser)]
#[command(
    name = "avito-rs",
    version,
    about = "Avito listing monitor (Rust port of parser_avito)"
)]
struct Cli {
    /// Подкоманда.
    #[command(subcommand)]
    command: Command,
}

/// Доступные команды.
#[derive(Debug, Subcommand)]
enum Command {
    /// Запустить монитор (вечный цикл, Ctrl+C останавливает).
    Run {
        /// Путь к конфигу.
        #[arg(short, long, default_value = "config.toml")]
        config: std::path::PathBuf,
        /// Один проход и выход (для отладки).
        #[arg(long)]
        once: bool,
    },
    /// Показать состояние БД и сводку URL.
    Status {
        /// Путь к конфигу.
        #[arg(short, long, default_value = "config.toml")]
        config: std::path::PathBuf,
    },
    /// REST API управления монитором (требует feature `api`).
    #[cfg(feature = "api")]
    Serve {
        /// Путь к конфигу.
        #[arg(short, long, default_value = "config.toml")]
        config: std::path::PathBuf,
        /// Адрес прослушивания: `host:port`.
        #[arg(short, long, default_value = "127.0.0.1:8080")]
        bind: String,
    },
    /// Проверить конфиг без запуска монитора.
    ValidateConfig {
        /// Путь к конфигу.
        #[arg(short, long, default_value = "config.toml")]
        config: std::path::PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { config, once } => run(config, once),
        #[cfg(feature = "api")]
        Command::Serve { config, bind } => serve(config, bind),
        Command::Status { config } => status(&config),
        Command::ValidateConfig { config } => validate(&config),
    }
}

/// Загрузить конфиг: файл или дефолт, если это `config.toml` по умолчанию.
fn load_config(path: &std::path::Path) -> Result<AppConfig> {
    if !path.exists() {
        if path.to_string_lossy().ends_with("config.toml") {
            return Ok(AppConfig::default());
        }
        anyhow::bail!("config not found: {}", path.display());
    }
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let cfg: AppConfig =
        toml::from_str(&raw).with_context(|| format!("parse config {}", path.display()))?;
    Ok(cfg)
}

/// `validate-config`: разбор + проверки инвариантов.
fn validate(path: &std::path::Path) -> Result<()> {
    let cfg = load_config(path)?;
    let redactor = Redactor::from_config(&cfg);

    let extractor_kind =
        extractors::ExtractorKind::parse(&cfg.extractor).map_err(|e| anyhow::anyhow!("{e}"))?;

    if cfg.urls.is_empty() {
        eprintln!("WARN: no urls configured — nothing to monitor");
    }
    if cfg.req_per_min == 0 {
        anyhow::bail!("[app] req_per_min must be > 0");
    }
    if cfg.pages_per_url == 0 {
        anyhow::bail!("[app] pages_per_url must be >= 1");
    }
    if cfg.filters.min_price > 0
        && cfg
            .filters
            .max_price
            .is_some_and(|m| cfg.filters.min_price > m)
    {
        anyhow::bail!("[filters] min_price > max_price");
    }
    if cfg.export.format != "csv" && cfg.export.format != "xlsx" {
        anyhow::bail!("[export] format must be `csv` or `xlsx`");
    }
    if cfg.security.api_key.is_empty() {
        eprintln!("WARN: [security] api_key empty — API auth disabled");
    }

    println!(
        "config OK: {} url(s), db={}",
        cfg.urls.len(),
        redactor.redact(&cfg.db_path.display().to_string())
    );
    println!(
        "  fetch_mode={} extractor={} (kind={extractor_kind:?})",
        cfg.fetch_mode, cfg.extractor
    );
    println!(
        "  interval={}s rpm={} pages_per_url={} max_concurrent={}",
        cfg.interval_sec, cfg.req_per_min, cfg.pages_per_url, cfg.max_concurrent
    );
    for (k, v) in cfg.flat_for_log() {
        println!("  {k} = {v}");
    }
    Ok(())
}

/// `status`: состояние БД + сводка.
fn status(path: &std::path::Path) -> Result<()> {
    let cfg = load_config(path)?;
    let store = SqliteStore::open(&cfg.db_path)?;
    println!(
        "db: {} (rows: {})",
        cfg.db_path.display(),
        store.seen_count()?
    );
    println!("urls: {}", cfg.urls.len());
    for u in &cfg.urls {
        println!("  {}", u.value);
    }
    Ok(())
}

/// `serve`: HTTP API без запуска цикла (монитором управляют через `/start`).
#[cfg(feature = "api")]
fn serve(path: std::path::PathBuf, bind: String) -> Result<()> {
    let cfg = load_config(&path)?;
    observability::init_logging(&cfg.observability)?;
    let metrics = Arc::new(Metrics::default());
    let state = Arc::new(api::server::ApiState::new(
        Arc::new(cfg.clone()),
        Arc::clone(&metrics),
    ));
    let key = cfg.security.api_key.clone();
    let router = api::server::router(state, key);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&bind)
            .await
            .with_context(|| format!("bind {bind}"))?;
        tracing::info!("api listening on {bind}");
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .context("api server")?;
        Ok(())
    })
}

/// `run`: запустить монитор.
fn run(path: std::path::PathBuf, once: bool) -> Result<()> {
    let mut cfg = load_config(&path)?;
    if once {
        cfg.one_shot = true;
        cfg.pages_per_url = cfg.pages_per_url.max(1);
    }
    observability::init_logging(&cfg.observability)?;

    if cfg.urls.is_empty() {
        anyhow::bail!(
            "no urls configured — add `urls = [...]` to {}",
            path.display()
        );
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(monitor_main(cfg, path))
}

/// Собрать экспортёр результатов (порт `build_result_storage`).
///
/// `format = "xlsx"` требует feature `xlsx` (без него → предупреждение и CSV).
fn build_exporter(cfg: &AppConfig) -> Result<Arc<dyn exporters::ResultExporter>> {
    let xlsx = cfg.export.format.eq_ignore_ascii_case("xlsx");
    if xlsx {
        #[cfg(feature = "xlsx")]
        {
            return Ok(Arc::new(exporters::xlsx::XlsxExporter::new(
                exporters::xlsx::single_path(&cfg.export.dir),
            )));
        }
        #[cfg(not(feature = "xlsx"))]
        {
            tracing::warn!("export format `xlsx` requested but the feature is off; using csv");
        }
    }
    let _ = xlsx;
    Ok(Arc::new(exporters::csv::CsvExporter::new(
        exporters::csv::single_path(&cfg.export.dir, "csv"),
    )))
}
/// Тело монитора (async).
async fn monitor_main(cfg: AppConfig, config_path: std::path::PathBuf) -> Result<()> {
    let cfg = Arc::new(cfg);
    let redactor = Arc::new(Redactor::from_config(&cfg));
    let metrics = Arc::new(Metrics::new());
    let store = Arc::new(SqliteStore::open(&cfg.db_path)?);
    let exporter: Arc<dyn exporters::ResultExporter> = build_exporter(&cfg)?;
    let fetcher = Arc::new(AvitoClient::new(Arc::clone(&cfg), Arc::clone(&redactor))?);

    tracing::info!(
        config = %redactor.redact(&config_path.display().to_string()),
        exporter = %exporter.target(),
        urls = cfg.urls.len(),
        "monitor starting"
    );

    let run_id = store.start_run()?;
    let monitor = Monitor::new(
        Arc::clone(&cfg),
        Arc::clone(&fetcher),
        Arc::clone(&store),
        Arc::clone(&exporter),
        Arc::clone(&metrics),
    );

    let tg = Arc::new(providers::telegram::TelegramNotifier::from_config(
        &cfg,
        Arc::clone(&redactor),
    )?);
    let vk = Arc::new(providers::vk::VkNotifier::from_config(
        &cfg,
        (*redactor).clone(),
    )?);

    let notify = {
        let tg = Arc::clone(&tg);
        let vk = Arc::clone(&vk);
        move |n: &avito_rs::dto::Notification| {
            let body = n.body.clone();
            let tg = Arc::clone(&tg);
            let vk = Arc::clone(&vk);
            tokio::spawn(async move {
                if let Some(t) = tg.as_ref() {
                    if let Err(e) = t.send_text(&body).await {
                        tracing::warn!("telegram notify failed: {e}");
                    }
                }
                if let Some(v) = vk.as_ref() {
                    if let Err(e) = v.send_text(&body).await {
                        tracing::warn!("vk notify failed: {e}");
                    }
                }
            });
        }
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let sig_tx = shutdown_tx.clone();
    let signal_task = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("SIGINT received — finishing current page before exit");
        let _ = sig_tx.send(true);
    });

    let n = Arc::new(notify);
    let n_first = Arc::clone(&n);
    let report = monitor.run_once(move |x| n_first(x)).await;
    store.finish_run(run_id, report.pages, report.new_ads, report.errors)?;
    tracing::info!(?report, metrics = ?metrics.snapshot(), "cycle finished");

    if !cfg.one_shot {
        monitor.run_forever(move |x| n(x), shutdown_rx).await;
    }

    signal_task.abort();
    tracing::info!("monitor stopped");
    println!(
        "done: pages={}, new_ads={}, errors={}",
        report.pages, report.new_ads, report.errors
    );
    Ok(())
}
