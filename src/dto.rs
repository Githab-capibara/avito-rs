//! Типизированные схемы (DTO) всего приложения.
//!
//! Единственный источник правды для: `Listing` (объявление), `Notification`
//! (сообщение в Telegram/VK), `AppConfig` (конфиг), `FilterCfg`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Одно объявление Авито в нормализованном виде.
///
/// Порт одноимённой pydantic-модели `Item` из `models.py` оригинала: все
/// необязательные поля отсутствуют в данных вместо того, чтобы ронять парсинг.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    /// Идентификатор объявления. В оригинале приходит как `int | dict | None`;
    /// здесь нормализован в строку (`String` или числовая строка).
    pub id: String,
    /// Заголовок.
    #[serde(default)]
    pub title: Option<String>,
    /// Короткое описание из списка.
    #[serde(default)]
    pub description: Option<String>,
    /// Цена в рублях (`priceDetailed.value`). `None` — «не указана».
    #[serde(default)]
    pub price: Option<u64>,
    /// Относительный путь объявления (`urlPath`) → абсолютный URL.
    #[serde(default)]
    pub url_path: Option<String>,
    /// Метка публикации, миллисекунды Unix (`sortTimeStamp`).
    #[serde(default)]
    pub sort_time_stamp: Option<i64>,
    /// Продавец — slug из `/brands/<slug>` или числовой `sellerId`.
    #[serde(default)]
    pub seller_id: Option<String>,
    /// Человекочитаемый адрес (`geo.formattedAddress`).
    #[serde(default)]
    pub formatted_address: Option<String>,
    /// Забронировано ли объявление (`isReserved`).
    #[serde(default)]
    pub is_reserved: bool,
    /// Продвигается ли объявление (`iva.DateInfoStep[].payload.vas[].title == "Продвинуто"`).
    #[serde(default)]
    pub is_promotion: bool,
    /// Просмотры всего (из карточки объявления, enrichment).
    #[serde(default)]
    pub total_views: Option<u64>,
    /// Просмотры сегодня (из карточки объявления, enrichment).
    #[serde(default)]
    pub today_views: Option<u64>,
    /// Первая фотография (для уведомлений с картинкой).
    #[serde(default)]
    pub first_image_url: Option<String>,
    /// Все изображения объявления (`Photo.root` в оригинале, `>`-размер → URL).
    #[serde(default)]
    pub images: Vec<String>,
    /// Координаты (`coords`: lat/lng/address_user из mobile API).
    #[serde(default)]
    pub coords: Option<serde_json::Value>,
    /// Телефон продавца (enrichment через `parse_phone`).
    #[serde(default)]
    pub phone: Option<String>,
}

impl Listing {
    /// Абсолютный URL объявления.
    #[must_use]
    pub fn url(&self) -> String {
        match self.url_path.as_deref() {
            Some(path) if path.starts_with("http") => path.to_owned(),
            Some(path) => format!("https://www.avito.ru{path}"),
            None => format!("https://www.avito.ru/{}", self.id),
        }
    }

    /// Текст для keyword-фильтров: `(title + description).to_lowercase()`.
    #[must_use]
    pub fn search_text(&self) -> String {
        format!(
            "{}{}",
            self.title.as_deref().unwrap_or_default(),
            self.description.as_deref().unwrap_or_default()
        )
        .to_lowercase()
    }
}

/// Уведомление, готовое к отправке в провайдер (Telegram / VK).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Заголовок сообщения.
    pub title: String,
    /// Основной текст.
    pub body: String,
    /// Ссылка на объявление.
    pub link: String,
    /// URL картинки, если есть.
    pub image_url: Option<String>,
    /// Признак «продвинуто» — идёт в текст пометкой.
    pub promoted: bool,
}

impl Notification {
    /// Собрать уведомление из объявления (поведение `Notifier.format_ad`).
    #[must_use]
    pub fn from_listing(listing: &Listing, only_text: bool) -> Self {
        let promoted = listing.is_promotion;
        let mut body = String::new();
        if let Some(price) = listing.price {
            body.push_str(&format!("💰 {price} ₽"));
            if promoted {
                body.push_str(" 🔥");
            }
            body.push('\n');
        }
        if let Some(title) = &listing.title {
            body.push_str(&format!("📦 {title}\n"));
        }
        if let Some(seller) = &listing.seller_id {
            body.push_str(&format!("👤 {seller}\n"));
        }
        if let Some(address) = &listing.formatted_address {
            body.push_str(&format!("📍 {address}\n"));
        }
        body.push_str(&format!("🔗 {}", listing.url()));
        let _ = only_text;
        Self {
            title: listing
                .title
                .clone()
                .unwrap_or_else(|| "Новое объявление".to_owned()),
            body,
            link: listing.url(),
            image_url: if only_text {
                None
            } else {
                listing.first_image_url.clone()
            },
            promoted,
        }
    }
}

/// Режим получения данных с Авито.
///
/// Порт фактического поведения оригинала: чистого «JSON-first из page HTML»
/// в оригинале нет — основной путь идёт через конвертер URL → mobile JSON API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchMode {
    /// URL → mobile JSON API через сторонний конвертер (как `spfa.pro` в оригинале).
    #[default]
    SpfaUrl,
    /// GET page HTML + извлечение JSON из `<script data-mfe-state="true">`.
    DirectHtml,
    /// Прямой mobile API (`m.avito.ru/api/1/card/items/{id}`, поисковый URL).
    MobileApi,
}

impl std::fmt::Display for FetchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::SpfaUrl => "spfa_url",
            Self::DirectHtml => "direct_html",
            Self::MobileApi => "mobile_api",
        };
        f.write_str(s)
    }
}

/// Один отслеживаемый URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrackedUrl {
    /// Отслеживаемый URL каталога.
    pub value: String,
}

impl<'de> serde::Deserialize<'de> for TrackedUrl {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self { value })
    }
}

impl From<String> for TrackedUrl {
    fn from(value: String) -> Self {
        Self { value }
    }
}

impl std::fmt::Display for TrackedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.value)
    }
}

/// Настройки фильтров (`[filters]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterCfg {
    /// Чёрный список слов: объявление отбрасывается, если слово встречается.
    #[serde(default)]
    pub keywords_black: Vec<String>,
    /// Белый список слов: остаётся только если хоть одно слово встречается.
    #[serde(default)]
    pub keywords_white: Vec<String>,
    /// Фильтр по адресу — подстрока в `geo.formattedAddress`.
    #[serde(default)]
    pub region: Option<String>,
    /// Чёрный список продавцов.
    #[serde(default)]
    pub sellers_blacklist: Vec<String>,
    /// Максимальная цена, руб.
    #[serde(default)]
    pub max_price: Option<u64>,
    /// Минимальная цена, руб.
    #[serde(default)]
    pub min_price: u64,
    /// Максимальный возраст объявления, секунды (`0` — без ограничения).
    #[serde(default)]
    pub max_age_sec: u64,
    /// Игнорировать забронированные.
    #[serde(default = "default_true")]
    pub ignore_reserved: bool,
    /// Отбрасывать продвигаемые.
    #[serde(default)]
    pub ignore_promotion: bool,
    /// Отсеивать уже виденные по паре (id, price) — поведение оригинала.
    #[serde(default = "default_true")]
    pub skip_seen: bool,
}

const fn default_true() -> bool {
    true
}

impl FilterCfg {
    /// Пустой фильтр (всё проходит) — для тестов и дефолтного конфига.
    #[must_use]
    pub const fn allow_all() -> Self {
        Self {
            keywords_black: Vec::new(),
            keywords_white: Vec::new(),
            region: None,
            sellers_blacklist: Vec::new(),
            max_price: None,
            min_price: 0,
            max_age_sec: 0,
            ignore_reserved: false,
            ignore_promotion: false,
            skip_seen: true,
        }
    }
}

/// Настройки уведомлений (`[notifications]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationsCfg {
    #[serde(default)]
    pub telegram_bot_token: Option<String>,
    #[serde(default)]
    pub telegram_chat_ids: Vec<i64>,
    /// Отправлять без картинок.
    #[serde(default)]
    pub telegram_only_text: bool,
    /// Прокси для Telegram (`None` — без прокси).
    #[serde(default)]
    pub telegram_proxy: Option<String>,
    #[serde(default)]
    pub vk_token: Option<String>,
    #[serde(default)]
    pub vk_peer_ids: Vec<i64>,
}

/// Настройки прокси (`[proxy]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyCfg {
    /// Список `http://user:pass@host:port`.
    #[serde(default)]
    pub list: Vec<String>,
    /// URL смены IP (мобильный прокси).
    #[serde(default)]
    pub change_ip_url: Option<String>,
}

/// Настройки стороннего сервиса cookies (`[cookie_service]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieServiceCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    /// Пауза между покупками cookies, сек (оригинал: `purchase_cooldown`).
    #[serde(default = "default_purchase_cooldown")]
    pub purchase_cooldown_sec: u64,
}

const fn default_purchase_cooldown() -> u64 {
    600
}

impl Default for CookieServiceCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "https://spfa.pro/api".to_owned(),
            api_key: String::new(),
            purchase_cooldown_sec: default_purchase_cooldown(),
        }
    }
}

/// Настройки экспорта (`[export]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportCfg {
    #[serde(default = "default_out_dir")]
    pub dir: PathBuf,
    /// `csv` всегда; `xlsx` — при feature `xlsx`.
    #[serde(default = "default_format")]
    pub format: String,
    /// Отдельный файл на каждый URL (как `one_file_for_link`).
    #[serde(default)]
    pub one_file_per_link: bool,
}

fn default_out_dir() -> PathBuf {
    PathBuf::from("./out")
}

fn default_format() -> String {
    "csv".to_owned()
}

impl Default for ExportCfg {
    fn default() -> Self {
        Self {
            dir: default_out_dir(),
            format: default_format(),
            one_file_per_link: false,
        }
    }
}

/// Настройки безопасности (`[security]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityCfg {
    /// API-ключ для REST (`X-Api-Key`), пусто = проверка отключена.
    #[serde(default)]
    pub api_key: String,
    /// Скрывать секреты в логах.
    #[serde(default = "default_true")]
    pub redact_secrets: bool,
}

impl Default for SecurityCfg {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            redact_secrets: true,
        }
    }
}

/// Настройки наблюдаемости (`[observability]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservabilityCfg {
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
    #[serde(default = "default_keep_days")]
    pub log_keep_days: u64,
    /// Максимум файлов логов (как `log_max_files`).
    #[serde(default = "default_max_files")]
    pub log_max_files: u64,
}

fn default_log_level() -> String {
    "info".to_owned()
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("./logs")
}

const fn default_keep_days() -> u64 {
    5
}

const fn default_max_files() -> u64 {
    10
}

impl Default for ObservabilityCfg {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            log_dir: default_log_dir(),
            log_keep_days: default_keep_days(),
            log_max_files: default_max_files(),
        }
    }
}

/// Корневая структура конфига (`config.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    /// Интервал полного цикла обхода, сек.
    #[serde(default = "default_interval")]
    pub interval_sec: u64,
    /// Сколько URL обрабатывать параллельно.
    #[serde(default = "default_concurrent")]
    pub max_concurrent: usize,
    /// Лимит запросов в минуту к Авито.
    #[serde(default = "default_rpm")]
    pub req_per_min: u32,
    /// Таймаут одного запроса, сек.
    #[serde(default = "default_timeout")]
    pub timeout_sec: u64,
    /// Сколько страниц пагинации обходить на URL (`count` в оригинале).
    #[serde(default = "default_pages")]
    pub pages_per_url: u32,
    /// Пауза между запросами страниц, сек (`pause_between_links`).
    #[serde(default = "default_pause_links")]
    pub pause_between_links_sec: u64,
    /// Сколько плохих ответов подряд → ротация прокси/cookies (`block_threshold`).
    #[serde(default = "default_block_threshold")]
    pub block_threshold: u32,
    /// Количество ретраев запроса (`max_count_of_retry`).
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Базовая задержка ретрая, сек (`retry_delay`).
    #[serde(default = "default_retry_delay")]
    pub retry_delay_sec: u64,
    /// Один проход и выход (`one_time_start`).
    #[serde(default)]
    pub one_shot: bool,
    /// Обогащать объявления просмотрами/описанием из карточки (`parse_views`).
    #[serde(default)]
    pub parse_views: bool,
    /// Загружать полное описание из карточки (`parse_description`).
    #[serde(default)]
    pub parse_description: bool,
    /// Режим получения данных.
    #[serde(default)]
    pub fetch_mode: FetchMode,
    /// Подмена хоста Авито (для тестов/прокси): `http://127.0.0.1:18080`.
    /// В оригинале нет — добавлено для возможности e2e-прогонов.
    #[serde(default)]
    pub api_base_override: Option<String>,
    /// Использовать cookies из файла `storage/own_cookies.json`.
    #[serde(default)]
    pub use_own_cookies: bool,
    /// Файл с cookies (JSON).
    #[serde(default = "default_cookies_path")]
    pub own_cookies_path: PathBuf,
    /// Путь к SQLite.
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
    /// Отслеживаемые URL.
    #[serde(default)]
    pub urls: Vec<TrackedUrl>,
    #[serde(default)]
    pub filters: FilterCfg,
    #[serde(default)]
    pub notifications: NotificationsCfg,
    #[serde(default)]
    pub proxy: ProxyCfg,
    #[serde(default)]
    pub cookie_service: CookieServiceCfg,
    #[serde(default)]
    pub export: ExportCfg,
    #[serde(default)]
    pub security: SecurityCfg,
    #[serde(default)]
    pub observability: ObservabilityCfg,
    /// Версия экстрактора (`v1_json` | `v2_html`).
    #[serde(default = "default_extractor")]
    pub extractor: String,
}

fn default_interval() -> u64 {
    60
}
fn default_concurrent() -> usize {
    5
}
fn default_rpm() -> u32 {
    30
}
fn default_timeout() -> u64 {
    30
}
fn default_pages() -> u32 {
    1
}
fn default_pause_links() -> u64 {
    5
}
fn default_block_threshold() -> u32 {
    3
}
fn default_retries() -> u32 {
    5
}
fn default_retry_delay() -> u64 {
    5
}
fn default_cookies_path() -> PathBuf {
    PathBuf::from("storage/own_cookies.json")
}
fn default_db_path() -> PathBuf {
    PathBuf::from("database.db")
}
fn default_extractor() -> String {
    "v1_json".to_owned()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            interval_sec: default_interval(),
            max_concurrent: default_concurrent(),
            req_per_min: default_rpm(),
            timeout_sec: default_timeout(),
            pages_per_url: default_pages(),
            pause_between_links_sec: default_pause_links(),
            block_threshold: default_block_threshold(),
            max_retries: default_retries(),
            retry_delay_sec: default_retry_delay(),
            one_shot: false,
            parse_views: false,
            parse_description: false,
            fetch_mode: FetchMode::default(),
            api_base_override: None,
            use_own_cookies: false,
            own_cookies_path: default_cookies_path(),
            db_path: default_db_path(),
            urls: Vec::new(),
            filters: FilterCfg::default(),
            notifications: NotificationsCfg::default(),
            proxy: ProxyCfg::default(),
            cookie_service: CookieServiceCfg::default(),
            export: ExportCfg::default(),
            security: SecurityCfg::default(),
            observability: ObservabilityCfg::default(),
            extractor: default_extractor(),
        }
    }
}

impl AppConfig {
    /// Плоский вид для security-лога/`status` (`key = value`), без секретов.
    #[must_use]
    pub fn flat_for_log(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("interval_sec".to_owned(), self.interval_sec.to_string());
        m.insert("max_concurrent".to_owned(), self.max_concurrent.to_string());
        m.insert("req_per_min".to_owned(), self.req_per_min.to_string());
        m.insert("timeout_sec".to_owned(), self.timeout_sec.to_string());
        m.insert("fetch_mode".to_owned(), self.fetch_mode.to_string());
        m.insert("urls".to_owned(), self.urls.len().to_string());
        m.insert("db_path".to_owned(), self.db_path.display().to_string());
        m
    }
}
