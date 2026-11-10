//! Экстракторы — «промпты» проекта: версионированные правила извлечения.
//!
//! Avito меняет разметку чаще, чем бизнес-логика. Поэтому правила извлечения
//! вынесены в отдельный модуль с версионированием, а сломанный экстрактор
//! страхуется golden-тестами (`evals/golden.rs`), а не только unit-тестами.
//!
//! Контракт [`Extractor`] одинаков для всех версий: на вход — сырой ответ
//! fetcher'а, на выход — `Vec<Listing>`. Непарсимое поле объявления даёт
//! `None`, а не ошибку (поведение pydantic-опциональных полей оригинала).

pub mod v1_json;

#[cfg(feature = "html-fallback")]
pub mod v2_html;

use crate::dto::Listing;
use sonic_rs::JsonValueTrait;

/// Ошибки извлечения (не могут уронить монитор — только пометить страницу битой).
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// Ответ не содержит JSON вообще.
    #[error("no JSON payload in response")]
    NoJson,
    /// JSON есть, но форма неожиданная (не найдено `items`).
    #[error("no `items` array in payload ({origin})")]
    NoItems {
        /// Где искали — для диагностики в логах.
        origin: String,
    },
    /// Синтаксическая ошибка JSON.
    #[error("malformed JSON: {0}")]
    MalformedJson(String),
}

/// Версия экстрактора — выбор в конфиге (`extractor = "v1_json" | "v2_html"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExtractorKind {
    /// JSON из `<script>`/API-ответа, serde-структуры.
    #[default]
    V1Json,
    /// CSS-селекторы по HTML (feature `html-fallback`).
    V2Html,
}

impl ExtractorKind {
    /// Распарсить значение конфига. Неизвестное → ошибка с подсказкой.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "v1_json" | "v1-json" | "v1" => Ok(Self::V1Json),
            "v2_html" | "v2-html" | "v2" => Ok(Self::V2Html),
            other => Err(format!(
                "unknown extractor `{other}`, expected `v1_json` or `v2_html`"
            )),
        }
    }
}

/// Трейт-порт: чистый разбор страницы → объявления. Без сети, без ввода-вывода.
pub trait Extractor: Send + Sync {
    /// Версия правила.
    fn kind(&self) -> ExtractorKind;

    /// Извлечь объявления. Ошибка — только если страница нечитаема в principe.
    fn extract(&self, page: &FetchedPage) -> Result<Vec<Listing>, ExtractError>;
}

use crate::providers::FetchedPage;

/// Активный экстрактор по конфигу.
#[must_use]
pub fn active(cfg_kind: ExtractorKind) -> Box<dyn Extractor> {
    match cfg_kind {
        ExtractorKind::V1Json => Box::new(v1_json::V1JsonExtractor),
        ExtractorKind::V2Html => {
            #[cfg(feature = "html-fallback")]
            {
                Box::new(v2_html::V2HtmlExtractor)
            }
            #[cfg(not(feature = "html-fallback"))]
            {
                // Мягкая деградация: без фичи падаем на JSON-экстрактор.
                tracing::warn!(
                    "extractor=v2_html requested but feature `html-fallback` is off; using v1_json"
                );
                Box::new(v1_json::V1JsonExtractor)
            }
        }
    }
}

/// Достать из payload'а каталог с `items`, обходя все обёртки оригинала.
///
/// Точный порт `AvitoParse._extract_api_catalog`: payload → result → catalog
/// → result → payload, первый объект с непустым `items`.
#[must_use]
pub fn find_items_node(payload: &sonic_rs::Value) -> Option<&sonic_rs::Value> {
    let candidates = [
        payload.get("catalog"),
        payload
            .get("result")
            .and_then(|r| r.get("catalog"))
            .filter(|_| payload.get("result").is_some()),
        payload.get("result"),
        Some(payload),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|c| c.get("items").is_some_and(|i| i.is_array()))
}

/// Хелпер для тестов/golden: извлечь объявления как JSON-строку.
pub fn extract_from_json_str(kind: ExtractorKind, raw: &str) -> Result<Vec<Listing>, ExtractError> {
    let page = FetchedPage {
        url: "fixture".to_owned(),
        status: 200,
        body: sonic_rs::from_str(raw).map_err(|e| ExtractError::MalformedJson(e.to_string()))?,
        raw_body: raw.as_bytes().to_vec(),
    };
    active(kind).extract(&page)
}
