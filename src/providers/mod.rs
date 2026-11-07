//! Провайдеры — единственное место с сетью (`providers/`).
//!
//! Всё остальное — чистые функции и хранилище. Фетчер отдаёт распарсенный
//! ответ и сырые байты; решение «как получать данные» (режим, прокси, cookies)
//! спрятано в [`AvitoClient`] за трейтом [`PageFetcher`].

pub mod avito;
pub mod backoff;
pub mod telegram;
pub mod vk;

/// Ответ fetcher'а: распарсенный JSON (если есть) + сырые байты для fallback.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// Запрошенный URL.
    pub url: String,
    /// HTTP-статус.
    pub status: u16,
    /// JSON-тело, если ответ провайдера его отдал (mobile API / конвертер).
    pub body: Option<serde_json::Value>,
    /// Сырые байты ответа.
    pub raw_body: Vec<u8>,
}

impl FetchedPage {
    /// Тело как строка (UTF-8 lossy).
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.raw_body).into_owned()
    }

    /// Хелпер для тестов: JSON-ответ.
    #[must_use]
    pub fn from_json(url: &str, body: serde_json::Value) -> Self {
        let raw = body.to_string().into_bytes();
        Self {
            url: url.to_owned(),
            status: 200,
            body: Some(body),
            raw_body: raw,
        }
    }

    /// Хелпер для тестов: сырые байты.
    #[must_use]
    pub fn from_bytes(url: &str, raw: Vec<u8>) -> Self {
        Self {
            url: url.to_owned(),
            status: 200,
            body: serde_json::from_slice(&raw).ok(),
            raw_body: raw,
        }
    }
}

/// Причина, по которой провайдер считает ответ блокировкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// 403 / 404 — нет доступа или ответа.
    Forbidden,
    /// 429 — слишком много запросов.
    TooManyRequests,
    /// 439 — спецкод Авито (файрвол QRATOR / проверка браузера).
    Fingerprint,
    /// Не flow-ошибка: детектор моря/фирмы сработал.
    BlockPage,
}

impl std::fmt::Display for BlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Forbidden => "403",
            Self::TooManyRequests => "429",
            Self::Fingerprint => "439",
            Self::BlockPage => "block-page",
        };
        f.write_str(s)
    }
}

/// Трейт-порт фетчера. Одна реализация (Avito), но тесты берут мок.
///
/// Нативный `async fn` в трейте (Rust ≥1.75) — без крейта `async-trait`.
pub trait PageFetcher: Send + Sync {
    /// Получить страницу/JSON.
    async fn fetch(&self, url: &str) -> anyhow::Result<FetchedPage>;

    /// Сообщить провайдеру, что ответ был признан блокировкой.
    async fn report_block(&self, _reason: BlockReason) {}
}

/// Выбор целевого режима fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// Список объявлений.
    Catalog,
    /// Карточка конкретного объявления.
    Item,
}
