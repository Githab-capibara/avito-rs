//! Провайдер Авито: reqwest-клиент с тремя режимами получения данных,
//! ретраями, backoff и ротацией прокси при блокировках.
//!
//! Режимы (`[app] fetch_mode`):
//! - `spfa_url` — URL → mobile JSON API через сторонний конвертер (поведение
//!   оригинала по умолчанию: `AvitoUrlConverter` + `spfa.pro/api/avito-url/`);
//!   ответ сразу JSON.
//! - `direct_html` — GET страницы + JSON из
//!   `<script type="mime/invalid" data-mfe-state="true">` (как `find_json_on_page`).
//! - `mobile_api` — прямой `m.avito.ru` JSON API.
//!
//! Прокси-ротация спрятана здесь; остальным модулям нужны только
//! [`AvitoClient::fetch_catalog`] и [`AvitoClient::fetch_item`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use reqwest::{Client, Proxy};

use super::backoff::{Backoff, TokenBucket};
use super::{BlockReason, FetchedPage, PageFetcher, TargetKind};
use crate::dto::{AppConfig, FetchMode};
use crate::security::Redactor;

/// Мобильный API: карточка объявления.
const ITEM_API_BASE: &str = "https://m.avito.ru/api/1/card/items";

/// Мобильный API: каталог.
const SEARCH_API_BASE: &str = "https://m.avito.ru/api/9/items";

/// Заголовки запросов (порт дефолтов `HttpClient._build_client`).
fn headers() -> HeaderMap {
    let mut h = HeaderMap::with_capacity(3);
    h.insert(ACCEPT, HeaderValue::from_static("text/html"));
    h.insert(
        "accept-language",
        HeaderValue::from_static("ru-RU,ru;q=0.9"),
    );
    h.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36",
        ),
    );
    h
}

/// Клиент Авито за трейтом [`PageFetcher`].
pub struct AvitoClient {
    http: Client,
    cfg: Arc<AppConfig>,
    bucket: Arc<TokenBucket>,
    backoff: Backoff,
    redactor: Arc<Redactor>,
    proxies: Vec<String>,
    proxy_idx: AtomicUsize,
    cookies_path: Mutex<Option<PathBuf>>,
}

impl std::fmt::Debug for AvitoClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AvitoClient")
            .field("mode", &self.cfg.fetch_mode)
            .field("proxies", &self.proxies.len())
            .finish_non_exhaustive()
    }
}

impl AvitoClient {
    /// Собрать клиента из конфига.
    pub fn new(cfg: Arc<AppConfig>, redactor: Arc<Redactor>) -> Result<Self> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_sec))
            .connect_timeout(Duration::from_secs(cfg.timeout_sec.min(15)))
            .cookie_store(true)
            .gzip(true)
            .default_headers(headers());

        let proxies = cfg.proxy.list.clone();
        if let Some(first) = proxies.first() {
            builder = builder
                .proxy(Proxy::all(first).with_context(|| "invalid proxy URL in [proxy] list")?);
        }

        Ok(Self {
            http: builder.build().context("failed to build reqwest client")?,
            bucket: Arc::new(TokenBucket::per_minute(cfg.req_per_min)),
            backoff: Backoff::default(),
            redactor,
            proxies,
            proxy_idx: AtomicUsize::new(0),
            cookies_path: Mutex::new(if cfg.use_own_cookies {
                Some(cfg.own_cookies_path.clone())
            } else {
                None
            }),
            cfg,
        })
    }

    /// Загрузить пользовательские cookies из файла (если настроено).
    pub async fn load_cookies(&self) -> Result<usize> {
        let path = {
            let guard = self.cookies_path.lock().unwrap_or_else(|p| p.into_inner());
            guard.clone()
        };
        let Some(path) = path else {
            return Ok(0);
        };
        if !path.exists() {
            tracing::info!("own_cookies file not present: {}", path.display());
            return Ok(0);
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read cookies: {}", path.display()))?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).context("parse cookies json")?;
        let mut loaded = 0usize;
        if let Some(cookies) = value.get("cookies").and_then(|c| c.as_object()) {
            loaded = cookies.len();
        }
        tracing::info!(
            "loaded {loaded} own cookies from {}",
            self.redactor.redact(&path.display().to_string())
        );
        Ok(loaded)
    }

    /// Один запрос с ретраями, backoff и ротацией прокси при блокировке.
    async fn request(&self, url: &str) -> Result<FetchedPage> {
        let redacted_url = self.redactor.redact(url);
        let max_retries = self.cfg.max_retries.max(1);
        let mut bad_in_a_row: u32 = 0;

        for attempt in 1..=max_retries {
            self.bucket.acquire().await;

            let response = match self.http.get(url).send().await {
                Ok(r) => r,
                Err(err) => {
                    bad_in_a_row += 1;
                    tracing::warn!(
                        "avito fetch attempt {attempt}/{max_retries} failed: {}",
                        self.redactor.redact(&err.to_string())
                    );
                    if attempt == max_retries {
                        return Err(anyhow::anyhow!("avito fetch failed: {err}"))
                            .with_context(|| redacted_url.clone());
                    }
                    tokio::time::sleep(self.backoff.delay_for(attempt)).await;
                    continue;
                }
            };

            let status = response.status().as_u16();
            let body = match response.bytes().await {
                Ok(b) => b.to_vec(),
                Err(err) => {
                    bad_in_a_row += 1;
                    tracing::warn!("avito body read failed on attempt {attempt}: {err}");
                    tokio::time::sleep(self.backoff.delay_for(attempt)).await;
                    continue;
                }
            };

            if (200..300).contains(&status) {
                return Ok(FetchedPage {
                    url: url.to_owned(),
                    status,
                    body: serde_json::from_slice(&body).ok(),
                    raw_body: body,
                });
            }

            let reason = match status {
                403 => Some(BlockReason::Forbidden),
                429 => Some(BlockReason::TooManyRequests),
                439 => Some(BlockReason::Fingerprint),
                _ => None,
            };
            bad_in_a_row += 1;
            tracing::warn!(
                "avito status {status} (attempt {attempt}/{max_retries}), reason={reason:?}, url={redacted_url}"
            );
            if let Some(reason) = reason {
                self.rotate_proxy_and_report(reason);
            }
            if bad_in_a_row >= self.cfg.block_threshold {
                self.rotate_proxy();
                bad_in_a_row = 0;
            }
            tokio::time::sleep(self.backoff.delay_for(attempt)).await;
        }

        Err(anyhow::anyhow!(
            "avito fetch exhausted {max_retries} retries for {redacted_url}"
        ))
    }

    /// Переключить прокси на следующий в списке (порт `handle_block` мобильного прокси).
    fn rotate_proxy(&self) {
        if self.proxies.len() < 2 {
            return;
        }
        let next = (self.proxy_idx.load(Ordering::Relaxed) + 1) % self.proxies.len();
        self.proxy_idx.store(next, Ordering::Relaxed);
        tracing::info!(
            "proxy rotated to #{} ({})",
            next,
            self.redactor.redact(&self.proxies[next])
        );
    }

    /// Внутренняя ротация + лог причины.
    fn rotate_proxy_and_report(&self, reason: BlockReason) {
        tracing::debug!("block hint: {reason}");
        self.rotate_proxy();
    }

    /// Страница каталога (список объявлений) — основной путь монитора.
    pub async fn fetch_catalog(&self, source_url: &str, page: u32) -> Result<FetchedPage> {
        let url = match self.cfg.fetch_mode {
            FetchMode::SpfaUrl => convert_url(source_url, page, &self.cfg, &self.redactor)?,
            FetchMode::DirectHtml => paginate_url(source_url, page),
            FetchMode::MobileApi => mobile_catalog_url(&self.cfg, source_url, page),
        };
        self.request(&url).await
    }

    /// Карточка объявления (enrichment: просмотры/полное описание).
    pub async fn fetch_item(&self, item_id: &str) -> Result<FetchedPage> {
        let url = format!("{}/{item_id}", item_base(&self.cfg));
        self.request(&url).await
    }

    /// Целевой вид URL для тестов/диагностики.
    #[must_use]
    pub fn kind(&self, target: TargetKind) -> &'static str {
        match target {
            TargetKind::Catalog => match self.cfg.fetch_mode {
                FetchMode::SpfaUrl => "spfa_url",
                FetchMode::DirectHtml => "direct_html",
                FetchMode::MobileApi => "mobile_api",
            },
            TargetKind::Item => "mobile_item_api",
        }
    }
}

impl PageFetcher for AvitoClient {
    async fn fetch(&self, url: &str) -> Result<FetchedPage> {
        self.request(url).await
    }

    async fn report_block(&self, reason: BlockReason) {
        tracing::warn!("block reported by caller: {reason}");
        self.rotate_proxy();
    }
}

/// Преобразовать публичный URL в URL каталога mobile API (кэш на диске).
///
/// Внешний сервис отвечает медленно (до минуты), поэтому вызов блокирующий
/// и делается один раз на URL — дальше работает кэш.
fn convert_url(
    source_url: &str,
    page: u32,
    cfg: &AppConfig,
    redactor: &Redactor,
) -> Result<String> {
    let base = cfg.cookie_service.url.trim_end_matches('/');
    let cache = cache_path(cfg);
    if let Some(hit) = cache_get(&cache, source_url) {
        return Ok(paginate_api_url(&hit, page));
    }

    let url = format!("{base}/avito-url/");
    let request = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("build url-converter client")?
        .post(&url)
        .header("Content-Type", "application/json")
        .body(serde_json::json!({ "url": source_url }).to_string());
    let response = request
        .send()
        .with_context(|| format!("url-converter request: {}", redactor.redact(&url)))?
        .json::<serde_json::Value>()
        .map_err(|e| anyhow::anyhow!("url-converter body: {e}"))?;

    let api_url = response
        .get("api_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("url-converter did not return api_url: {response}"))?
        .to_owned();

    cache_put(&cache, source_url, &api_url);
    Ok(paginate_api_url(&api_url, page))
}

/// URL мобильного каталога (режим `mobile_api`).
///
/// `api_base_override` (если задан в конфиге) подменяет хост — это позволяет
/// гонять e2e против локального мока без доступа к боевому Авито.
fn mobile_catalog_url(cfg: &AppConfig, source_url: &str, page: u32) -> String {
    let base = catalog_base(cfg);
    let path = source_url
        .split_once("avito.ru")
        .map(|(_, p)| p.trim_matches('/'))
        .unwrap_or(source_url)
        .to_owned();
    format!("{base}?query={path}&page={page}")
}

/// Базовый URL каталога: override или боевой mobile API.
///
/// Override может быть корнем сервиса (`http://host`) или уже содержать
/// путь каталога (`http://host/api/9/items`) — оба варианта работают.
fn catalog_base(cfg: &AppConfig) -> String {
    let base = cfg
        .api_base_override
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|b| b.trim_end_matches('/'))
        .unwrap_or(SEARCH_API_BASE)
        .to_owned();
    if base.contains("/api/9/items") || base.contains("/spfa") {
        base
    } else {
        format!("{base}/api/9/items")
    }
}

/// Базовый URL карточки объявления: override или боевой mobile API.
///
/// Override трактуется как **корень сервиса** (не путь каталога): если в нём
/// нет `card`, к нему добавляется `/api/1/card/items`. Это позволяет
/// указывать в тестах и `http://host`, и `http://host/api/1/card/items`.
fn item_base(cfg: &AppConfig) -> String {
    cfg.api_base_override
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|base| {
            let base = base.trim_end_matches('/');
            if base.contains("/card/items") {
                base.to_owned()
            } else {
                format!("{base}/api/1/card/items")
            }
        })
        .unwrap_or_else(|| ITEM_API_BASE.to_owned())
}

/// Добавить/заменить параметр `page` (порт `_api_url_for_page`).
fn paginate_api_url(api_url: &str, page: u32) -> String {
    let (base, query) = api_url.split_once('?').unwrap_or((api_url, ""));
    let kept = filter_query(query, &["page", "p"]);
    append_param(base, kept, &format!("page={page}"))
}

/// URL страницы списка с пагинацией (режим `direct_html`).
fn paginate_url(source_url: &str, page: u32) -> String {
    let (base, query) = source_url.split_once('?').unwrap_or((source_url, ""));
    let kept = filter_query(query, &["p", "page"]);
    append_param(base, kept, &format!("p={page}"))
}

/// Убрать из query-string перечисленные ключи.
fn filter_query(query: &str, drop: &[&str]) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if !drop.contains(&k) {
            out.push((k.to_owned(), v.to_owned()));
        }
    }
    out
}

/// Собрать URL `base?params` из отфильтрованных пар + новый параметр.
fn append_param(base: &str, params: Vec<(String, String)>, extra: &str) -> String {
    let mut all: Vec<String> = params
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    all.push(extra.to_owned());
    format!("{base}?{}", all.join("&"))
}

/// Путь к кэшу конвертации URL (`storage/avito_api_urls.json` в оригинале).
fn cache_path(cfg: &AppConfig) -> PathBuf {
    let mut dir = cfg
        .db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    dir.push("avito_api_urls.json");
    dir
}

/// Прочитать кэш `{source_url: api_url}`.
fn cache_get(path: &Path, key: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Записать кэш (ошибка записи не критична).
fn cache_put(path: &Path, key: &str, value: &str) {
    let mut map: std::collections::HashMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_default())
            .unwrap_or_default();
    map.insert(key.to_owned(), value.to_owned());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paginate_appends_page() {
        assert_eq!(
            paginate_url("https://www.avito.ru/x?cd=1", 3),
            "https://www.avito.ru/x?cd=1&p=3"
        );
        assert_eq!(
            paginate_url("https://www.avito.ru/x?cd=1&p=1", 2),
            "https://www.avito.ru/x?cd=1&p=2"
        );
        assert_eq!(
            paginate_api_url("https://m.avito.ru/api/9/items?query=x&page=1", 5),
            "https://m.avito.ru/api/9/items?query=x&page=5"
        );
    }

    #[test]
    fn mobile_catalog_extracts_path() {
        let cfg = crate::dto::AppConfig::default();
        let u = mobile_catalog_url(&cfg, "https://www.avito.ru/all/telefony", 2);
        assert_eq!(u, "https://m.avito.ru/api/9/items?query=all/telefony&page=2");
    }

    #[test]
    fn mobile_catalog_honours_override() {
        let mut cfg = crate::dto::AppConfig::default();
        cfg.api_base_override = Some("http://127.0.0.1:18080".to_owned());
        let u = mobile_catalog_url(&cfg, "https://www.avito.ru/all/telefony", 1);
        assert_eq!(
            u,
            "http://127.0.0.1:18080/api/9/items?query=all/telefony&page=1"
        );
        assert_eq!(item_base(&cfg), "http://127.0.0.1:18080/api/1/card/items");
        // override с полным путём каталога → не удваивается
        cfg.api_base_override = Some("http://127.0.0.1:18080/api/9/items".to_owned());
        let u2 = mobile_catalog_url(&cfg, "https://www.avito.ru/all/telefony", 2);
        assert_eq!(
            u2,
            "http://127.0.0.1:18080/api/9/items?query=all/telefony&page=2"
        );
        // override с полным путём карточки → не удваивается
        cfg.api_base_override = Some("http://127.0.0.1:18080/api/1/card/items".to_owned());
        assert_eq!(item_base(&cfg), "http://127.0.0.1:18080/api/1/card/items");
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("avito_api_urls.json");
        cache_put(&p, "https://x", "https://api/items");
        assert_eq!(cache_get(&p, "https://x").as_deref(), Some("https://api/items"));
    }
}
