//! Рабочие процессы: цикл монитора, конвейер фильтров, бюджеты.
//!
//! Итерация одного URL: fetch → extract → filters → дедуп (id, price) →
//! уведомления + экспорт + `mark_seen`. Результаты пишутся после каждой
//! страницы (как в оригинале), чтобы уже собранные объявления не потерялись
//! при ошибке на следующей странице.

pub mod enrich;
pub mod filters;

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::watch;

use crate::application::filters::apply;
use crate::dto::{AppConfig, Listing, Notification};
use crate::exporters::ResultExporter;
use crate::extractors::{active as active_extractor, ExtractError, ExtractorKind};
use crate::observability::Metrics;
use crate::providers::avito::AvitoClient;
use crate::storage::SqliteStore;

/// Счётчики цикла.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleReport {
    /// Обработано страниц.
    pub pages: u64,
    /// Новых объявлений после дедупа и фильтров.
    pub new_ads: u64,
    /// Ошибок за цикл.
    pub errors: u64,
}

/// Монитор: владеет клиентом, хранилищем, метриками.
pub struct Monitor {
    fetcher: Arc<AvitoClient>,
    store: Arc<SqliteStore>,
    exporter: Arc<dyn ResultExporter>,
    metrics: Arc<Metrics>,
    extractor_kind: ExtractorKind,
    cfg: Arc<RwLock<AppConfig>>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("extractor", &self.extractor_kind)
            .field("urls", &self.cfg.read().map(|c| c.urls.len()).unwrap_or(0))
            .finish_non_exhaustive()
    }
}

impl Monitor {
    /// Создать монитор из собранных зависимостей.
    pub fn new(
        cfg: Arc<AppConfig>,
        fetcher: Arc<AvitoClient>,
        store: Arc<SqliteStore>,
        exporter: Arc<dyn ResultExporter>,
        metrics: Arc<Metrics>,
    ) -> Self {
        let kind = ExtractorKind::parse(&cfg.extractor).unwrap_or_else(|msg| {
            tracing::error!("{msg}; falling back to v1_json");
            ExtractorKind::V1Json
        });
        Self {
            cfg: Arc::new(RwLock::new((*cfg).clone())),
            fetcher,
            store,
            exporter,
            metrics,
            extractor_kind: kind,
        }
    }

    /// Один полный цикл по всем URL.
    pub async fn run_once<F>(&self, notify: F) -> CycleReport
    where
        F: Fn(&Notification) + Send + Sync + 'static,
    {
        let extractor_kind = self.extractor_kind;
        // Читаем конфиг свежий на каждый цикл — позволяет горячему обновлению
        // через разделяемый RwLock (PUT /urls в API, если монитор и API в одном процессе).
        let cfg = Arc::new(
            self.cfg
                .read()
                .map(|g| g.clone())
                .unwrap_or_else(|e| e.into_inner().clone()),
        );
        let semaphore = Arc::new(tokio::sync::Semaphore::new(cfg.max_concurrent.max(1)));
        let notify = Arc::new(notify);
        let mut report = CycleReport::default();

        let mut tasks = Vec::with_capacity(cfg.urls.len());
        for url in &cfg.urls {
            let source = url.value.clone();
            let permit = Arc::clone(&semaphore);
            let cfg = Arc::clone(&cfg);
            let fetcher = Arc::clone(&self.fetcher);
            let store = Arc::clone(&self.store);
            let exporter = Arc::clone(&self.exporter);
            let metrics = Arc::clone(&self.metrics);
            let notify = Arc::clone(&notify);

            tasks.push(tokio::spawn(async move {
                let _permit = permit.acquire().await;
                process_url(
                    cfg,
                    fetcher,
                    store,
                    exporter,
                    metrics,
                    notify,
                    extractor_kind,
                    source,
                )
                .await
            }));
        }

        for handle in tasks {
            match handle.await {
                Ok(sub) => merge(&mut report, sub),
                Err(e) => {
                    report.errors += 1;
                    tracing::warn!("url worker panicked: {e}");
                }
            }
        }
        report
    }

    /// Вечный цикл пока не выставлен флаг остановки.
    pub async fn run_forever<F>(&self, notify: F, mut shutdown: watch::Receiver<bool>)
    where
        F: Fn(&Notification) + Send + Sync + 'static,
    {
        let notify = Arc::new(notify);
        loop {
            if *shutdown.borrow_and_update() {
                tracing::info!("shutdown flag set — monitor stops");
                return;
            }
            let cfg = self
                .cfg
                .read()
                .map(|c| c.clone())
                .unwrap_or_else(|e| e.into_inner().clone());
            let n = Arc::clone(&notify);
            let report = self.run_once(move |x| n(x)).await;
            tracing::info!(
                "cycle done: pages={}, new={}, errors={}",
                report.pages,
                report.new_ads,
                report.errors
            );
            if cfg.one_shot {
                tracing::info!("one_shot enabled — monitor stops after first cycle");
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(cfg.interval_sec)) => {}
                _ = shutdown.changed() => { tracing::info!("shutdown during idle"); return; }
            }
        }
    }
}

/// Обход одного URL: страницы → извлечение → фильтры → дедуп → уведомления.
#[allow(clippy::too_many_arguments)]
async fn process_url<F>(
    cfg: Arc<AppConfig>,
    fetcher: Arc<AvitoClient>,
    store: Arc<SqliteStore>,
    exporter: Arc<dyn ResultExporter>,
    metrics: Arc<Metrics>,
    notify: Arc<F>,
    extractor_kind: ExtractorKind,
    source_url: String,
) -> CycleReport
where
    F: Fn(&Notification) + Send + Sync + 'static,
{
    let extractor = active_extractor(extractor_kind);
    let mut report = CycleReport::default();

    for page in 1..=cfg.pages_per_url.max(1) {
        let span = tracing::debug_span!("fetch_page", page);
        let _enter = span.enter();

        let fetched = match fetcher.fetch_catalog(&source_url, page).await {
            Ok(f) => f,
            Err(e) => {
                report.errors += 1;
                metrics.inc_fetch_errors();
                tracing::warn!("fetch failed for {source_url} page {page}: {e}");
                break;
            }
        };
        report.pages += 1;
        metrics.inc_pages();

        let listings = match extractor.extract(&fetched) {
            Ok(l) => l,
            Err(ExtractError::NoItems { origin }) => {
                tracing::info!("no more listings for {source_url} page {page} ({origin})");
                break;
            }
            Err(e) => {
                report.errors += 1;
                metrics.inc_parse_errors();
                tracing::warn!("extract failed for {source_url} page {page}: {e}");
                continue;
            }
        };

        let filtered = apply(&cfg.filters, listings);
        tracing::debug!("{} listings after filters", filtered.len());

        let mut fresh: Vec<Listing> = Vec::new();
        if cfg.filters.skip_seen {
            let seen_flags = match store.is_seen_batch(&filtered) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("is_seen_batch failed: {e}");
                    report.errors += 1;
                    vec![false; filtered.len()]
                }
            };
            for (listing, seen) in filtered.into_iter().zip(seen_flags) {
                if seen {
                    continue;
                }
                fresh.push(listing);
            }
        } else {
            fresh = filtered;
        }

        if fresh.is_empty() {
            tracing::info!("no new listings for {source_url} page {page}");
            break;
        }

        let note_count = fresh.len();
        for ad in &fresh {
            let note = Notification::from_listing(ad, cfg.notifications.telegram_only_text);
            metrics.inc_new_ads();
            notify(&note);
        }
        report.new_ads += u64::try_from(note_count).unwrap_or(0);
        let concurrency = cfg.max_concurrent.max(1);
        let _ = crate::application::enrich::parse_views(
            &fetcher,
            &mut fresh,
            cfg.parse_views,
            cfg.parse_description,
            concurrency,
        )
        .await;
        let _ = crate::application::enrich::parse_description(
            &fetcher,
            &mut fresh,
            cfg.parse_views,
            cfg.parse_description,
            concurrency,
        )
        .await;

        if let Err(e) = exporter.save(&fresh) {
            tracing::warn!("export failed: {e}");
            report.errors += 1;
        }
        if let Err(e) = store.mark_seen_batch(&fresh) {
            tracing::warn!("mark_seen_batch failed: {e}");
            report.errors += 1;
        }
        // Отслеживание цены: фиксируем текущую цену для spätere оценки снижения.
        for ad in &fresh {
            if let Some(price) = ad.price {
                if let Err(e) = store.set_price(&ad.id, price) {
                    tracing::warn!("set_price failed for {}: {e}", ad.id);
                    report.errors += 1;
                }
            }
        }
        advisory_sleep(cfg.pause_between_links_sec).await;
    }

    report
}

/// Короткая пауза между страницами без блокировки event loop.
async fn advisory_sleep(secs: u64) {
    if secs == 0 {
        return;
    }
    tokio::time::sleep(Duration::from_millis(secs.min(2_000))).await;
}

/// Слить отчёт воркера в общий.
fn merge(total: &mut CycleReport, sub: CycleReport) {
    total.pages += sub.pages;
    total.new_ads += sub.new_ads;
    total.errors += sub.errors;
}
