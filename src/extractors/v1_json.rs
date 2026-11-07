//! Экстрактор `v1_json`: порт моделей `models.py` (154 поля `Item`)
//! и функций `_extract_api_catalog`, `_clean_null_ads`,
//! `_add_seller_to_ads`, `_add_promotion_to_ads` из `parser_cls.py`.
//!
//! Отличие от оригинала: валидация «всё или ничего» заменена на независимую
//! обработку каждой необязательной ветки, поэтому один сломанный элемент
//! не теряет весь каталог (в оригинале `ValidationError` ронял страницу в `continue`).

use serde::Deserialize;

use super::{ExtractError, Extractor, ExtractorKind};
use crate::dto::Listing;
use crate::providers::FetchedPage;

/// Корневой узел каталога (`ItemsResponse`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemsResponse {
    #[serde(default)]
    pub items: Vec<Item>,
}

/// Элемент каталога. Используемые поля; остальное оригинальной 154-полевой
/// схемы игнорируется — serde не требует исчерпывающего набора ключей.
///
/// API (и оригинальный `parser_avito`) отдаёт ключи в camelCase — фактический
/// ответ выше сериализуется в `urlPath`, `priceDetailed`, `sortTimeStamp`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// В оригинале `int | dict | None` — нормализуем в строку.
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url_path: Option<String>,
    #[serde(default)]
    pub sort_time_stamp: Option<i64>,
    #[serde(default)]
    pub price_detailed: Option<PriceDetailed>,
    #[serde(default)]
    pub normalized_price: Option<String>,
    #[serde(default)]
    pub is_reserved: Option<bool>,
    #[serde(default)]
    pub iva: Option<serde_json::Value>,
    #[serde(default)]
    pub geo: Option<Geo>,
    #[serde(default)]
    pub seller_id: Option<String>,
    #[serde(default)]
    pub images: Option<serde_json::Value>,
    /// `coords` (lat/lng/address_user) как в оригинале.
    #[serde(default)]
    pub coords: Option<serde_json::Value>,
}

/// `PriceDetailed` из оригинала.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceDetailed {
    #[serde(default)]
    pub value: Option<u64>,
    #[serde(default)]
    pub string: Option<String>,
}

/// `Geo` из оригинала.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Geo {
    #[serde(default)]
    pub formatted_address: Option<String>,
}

/// Экстрактор JSON-каталога.
#[derive(Debug, Clone, Copy, Default)]
pub struct V1JsonExtractor;

/// Достать число из строки вида `"5 500 ₽"` (цифры, пробелы игнорируются).
fn parse_price_text(s: &str) -> Option<u64> {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

impl V1JsonExtractor {
    /// Нормализовать `id` (`int | dict | None`) в строку.
    fn normalize_id(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(true) => Some("true".to_owned()),
            _ => None,
        }
    }

    /// Порт `_clean_null_ads`: отбросить объявления без id.
    fn is_valid(item: &Item) -> bool {
        item.id.as_ref().and_then(Self::normalize_id).is_some()
    }

    /// Порт `_extract_seller_slug`: `/brands/<slug>` в любом тексте поля.
    fn extract_seller_slug(raw: &str) -> Option<String> {
        let marker = "/brands/";
        let start = raw.find(marker)? + marker.len();
        let rest = &raw[start..];
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let slug = &rest[..end];
        (!slug.is_empty()).then(|| slug.to_owned())
    }

    /// Порт `_add_promotion_to_ads`: `iva.DateInfoStep[].payload.vas[].title == "Продвинуто"`.
    fn is_promoted(iva: &serde_json::Value) -> bool {
        let Some(steps) = iva.get("DateInfoStep").and_then(|s| s.as_array()) else {
            return false;
        };
        steps.iter().any(|step| {
            step.get("payload")
                .and_then(|p| p.get("vas"))
                .and_then(|v| v.as_array())
                .is_some_and(|vas| {
                    vas.iter()
                        .any(|v| v.get("title").and_then(|t| t.as_str()) == Some("Продвинуто"))
                })
        })
    }

    /// Все URL изображений (для колонки XLSX «Изображения»).
    #[must_use]
    pub fn all_images(images: &serde_json::Value) -> Vec<String> {
        images
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|entry| {
                        entry
                            .as_object()?
                            .values()
                            .find_map(|url| url.as_str())
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Порт `Photo`: `images[*].<*Url>` → первый URL картинки.
    fn first_image(images: &serde_json::Value) -> Option<String> {
        let arr = images.as_array()?;
        for entry in arr {
            if let Some(obj) = entry.as_object() {
                if let Some((_k, v)) = obj.iter().next() {
                    if let Some(s) = v.as_str() {
                        if s.starts_with("http") {
                            return Some(s.to_owned());
                        }
                    }
                }
            }
        }
        None
    }

    /// Конвертировать узел `items` в список объявлений (общий вход для
    /// `v1_json` и `v2_html`, оба парсят один и тот же каталог).
    ///
    /// # Errors
    /// Узел не десериализуется в `ItemsResponse`.
    pub fn extract_items(&self, node: &serde_json::Value) -> Result<Vec<Listing>, String> {
        let parsed: ItemsResponse =
            serde_json::from_value(node.clone()).map_err(|e| e.to_string())?;
        Ok(parsed
            .items
            .into_iter()
            .filter(Self::is_valid)
            .filter_map(Self::to_listing)
            .collect())
    }

    /// Конвертировать `Item` в `Listing`.
    fn to_listing(item: Item) -> Option<Listing> {
        let normalized_price = item.normalized_price.as_deref().and_then(parse_price_text);
        let id = item.id.as_ref().and_then(Self::normalize_id)?;
        let iva = item.iva.as_ref();
        let seller_from_slug = item
            .seller_id
            .as_ref()
            .map(|s| Self::extract_seller_slug(s).unwrap_or_else(|| s.clone()));
        Some(Listing {
            id,
            title: item.title.clone(),
            description: item.description.clone(),
            price: item
                .price_detailed
                .as_ref()
                .and_then(|p| p.value)
                .or(normalized_price),
            url_path: item.url_path.clone(),
            sort_time_stamp: item.sort_time_stamp,
            seller_id: seller_from_slug
                .or_else(|| item.seller_id.clone().filter(|s| !s.is_empty())),
            formatted_address: item.geo.as_ref().and_then(|g| g.formatted_address.clone()),
            is_reserved: item.is_reserved.unwrap_or(false),
            is_promotion: iva.is_some_and(Self::is_promoted),
            total_views: None,
            today_views: None,
            first_image_url: item.images.as_ref().and_then(Self::first_image),
            images: item
                .images
                .as_ref()
                .map(Self::all_images)
                .unwrap_or_default(),
            coords: item.coords.clone(),
            phone: None,
        })
    }
}

impl Extractor for V1JsonExtractor {
    fn kind(&self) -> ExtractorKind {
        ExtractorKind::V1Json
    }

    fn extract(&self, page: &FetchedPage) -> Result<Vec<Listing>, ExtractError> {
        let payload = page.body.as_ref().ok_or(ExtractError::NoJson)?;
        let items_node = super::find_items_node(payload).ok_or_else(|| ExtractError::NoItems {
            origin: page.url.clone(),
        })?;

        let parsed: ItemsResponse = serde_json::from_value(items_node.clone())
            .map_err(|e| ExtractError::MalformedJson(e.to_string()))?;

        let out: Vec<Listing> = parsed
            .items
            .into_iter()
            .filter(Self::is_valid)
            .filter_map(|item| match Self::to_listing(item) {
                Some(l) => Some(l),
                None => {
                    tracing::debug!("dropped malformed listing during extraction");
                    None
                }
            })
            .collect();
        Ok(out)
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn extracts_all_images_and_coords() {
        let node = serde_json::json!({"items": [{
            "id": 7,
            "images": [
                {"140x105": "https://a/1.jpg", "640x480": "https://a/1big.jpg"},
                {"140x105": "https://a/2.jpg"}
            ],
            "coords": {"lat": 45.0, "lng": 39.0, "address_user": "Krasnodar"}
        }]});
        let out = V1JsonExtractor.extract_items(&node).expect("items");
        assert_eq!(out.len(), 1);
        // первый URL из каждого объявления-объекта (порядок значений ObjectValues)
        assert_eq!(out[0].images.len(), 2);
        assert_eq!(out[0].coords.as_ref().unwrap()["address_user"], "Krasnodar");
        assert!(out[0].first_image_url.is_some());
    }

    #[test]
    fn priced_via_normalized_price_when_value_absent() {
        let node = serde_json::json!({"items": [{
            "id": "abc", "normalizedPrice": "5 500 ₽"
        }]});
        let out = V1JsonExtractor.extract_items(&node).expect("items");
        assert_eq!(out[0].price, Some(5500));
        assert_eq!(out[0].id, "abc");
    }

    #[test]
    fn promotion_detected_from_iva_steps() {
        let node = serde_json::json!({"items": [{
            "id": 1,
            "iva": {"DateInfoStep": [{"payload": {"vas": [
                {"title": "Продвинуто"}
            ]}}]}
        }]});
        let out = V1JsonExtractor.extract_items(&node).expect("items");
        assert!(out[0].is_promotion);
    }

    #[test]
    fn seller_slug_from_brands_path() {
        assert_eq!(
            V1JsonExtractor::extract_seller_slug("https://x/brands/foo-bar/baz"),
            Some("foo-bar".to_owned())
        );
        assert_eq!(
            V1JsonExtractor::extract_seller_slug("/brands/a?x=1"),
            Some("a".to_owned())
        );
        assert_eq!(V1JsonExtractor::extract_seller_slug("/brands/"), None);
    }
}
