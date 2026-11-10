//! Обогащение объявлений данными карточки (порт `_enrich_ads` из
//! `parser_cls.py`).
//!
//! Оригинал ходит в `m.avito.ru/api/1/card/items/{id}` и достаёт:
//! - просмотры: `success.mobile.stats.views.total|today`,
//!   фолбэк `success.view.scenario.beduin.main.params.metaDataAndStats.views`;
//! - полное описание: `success.mobile.description`,
//!   фолбэк `...params.description.segments[].text` (`\u2028` → `\n`).
//!
//! Пауза `random.uniform(0.1, 0.9)` между карточками — джиттер, чтобы не
//! ловить rate-limit.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sonic_rs::{JsonContainerTrait, JsonValueTrait};
use tokio::sync::Semaphore;

use crate::dto::Listing;
use crate::providers::avito::AvitoClient;

/// Порт `_extract_item_details`: (описание, всего, сегодня).
#[must_use]
pub fn extract_item_details(
    payload: &sonic_rs::Value,
) -> (Option<String>, Option<u64>, Option<u64>) {
    let mobile = nested(payload, &["success", "mobile"]);
    let mobile_views = nested(&mobile, &["stats", "views"]);
    let card_params = nested(
        payload,
        &["success", "view", "scenario", "beduin", "main", "params"],
    );
    let card_views = nested(&card_params, &["metaDataAndStats", "views"]);

    let total = count(mobile_views.get("total")).or_else(|| count(card_views.get("total")));
    let today = count(mobile_views.get("today")).or_else(|| {
        count(
            card_views
                .get("today")
                .or_else(|| card_views.get("todayViews"))
                .or_else(|| card_views.get("today_views")),
        )
    });

    let description = mobile
        .get("description")
        .and_then(|d| d.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| card_description(&card_params));

    (description, total, today)
}

/// Собрать описание из `segments[].text` (`\u2028` → `\n`).
fn card_description(card_params: &sonic_rs::Value) -> Option<String> {
    let node = nested(card_params, &["description"]);
    let arr = node.get("segments").and_then(|s| s.as_array())?;
    let text: String = arr
        .iter()
        .filter_map(|s| s.get("text").and_then(|t| t.as_str()))
        .collect();
    let text = text.replace('\u{2028}', "\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Порт `nested_dict`: дойти по ключам, вернуть `{}` при любом обрыве.
fn nested(value: &sonic_rs::Value, keys: &[&str]) -> sonic_rs::Value {
    let mut current: &sonic_rs::Value = value;
    for key in keys {
        match JsonValueTrait::get(current, key) {
            Some(next) => current = next,
            // Оригинал возвращает `{}`; здесь пустой объект даёт None для
            // последующих `get`, что для callers эквивалентно.
            None => return sonic_rs::Value::new_object(),
        }
    }
    current.clone()
}

/// Порт `count`: `bool` игнорируется, `int` берётся как есть, строка —
/// только цифры.
fn count(value: Option<&sonic_rs::Value>) -> Option<u64> {
    let value = value?;
    if value.is_boolean() {
        return None;
    }
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    if let Some(s) = value.as_str() {
        let digits: String = s.chars().filter(char::is_ascii_digit).collect();
        return digits.parse::<u64>().ok();
    }
    None
}

/// Порт `_enrich_ads`: обойти список, скачать карточки параллельно, заполнить поля.
///
/// `concurrency` ограничивает число одновременных запросов к карточкам.
pub async fn enrich(
    client: &Arc<AvitoClient>,
    ads: &mut [Listing],
    views: bool,
    _description: bool,
    concurrency: usize,
) -> Result<usize> {
    if ads.is_empty() {
        return Ok(0);
    }
    let concurrency = concurrency.max(1);
    let semaphore = Arc::new(Semaphore::new(concurrency));

    let work: Vec<(usize, u64)> = ads
        .iter()
        .enumerate()
        .filter_map(|(index, ad)| ad.id.parse::<u64>().ok().map(|id| (index, id)))
        .collect();

    let mut handles = Vec::with_capacity(work.len());
    for (index, id) in work {
        let client = Arc::clone(client);
        let sem = Arc::clone(&semaphore);
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            let page = match client.fetch_item(&id.to_string()).await {
                Ok(p) => p,
                Err(err) => {
                    tracing::warn!("card request failed for id {id}: {err}");
                    return None;
                }
            };
            let Some(payload) = page.body.as_ref() else {
                tracing::warn!("card payload for id {id} is not JSON");
                return None;
            };
            let (description, total_views, today_views) = extract_item_details(payload);
            let mut jitter = Jitter::default();
            tokio::time::sleep(Duration::from_secs_f64(jitter.next())).await;
            Some((index, description, total_views, today_views))
        }));
    }

    let mut enriched = 0usize;
    for handle in handles {
        if let Ok(Some((index, description, total_views, today_views))) = handle.await {
            if views {
                ads[index].total_views = total_views;
                ads[index].today_views = today_views;
            }
            if let Some(text) = description {
                ads[index].description = Some(text);
            }
            enriched += 1;
        }
    }
    Ok(enriched)
}

/// Джиттер `random.uniform(0.1, 0.9)` без крейта `rand`.
#[derive(Debug)]
struct Jitter {
    seed: u64,
}
impl Default for Jitter {
    fn default() -> Self {
        Self {
            seed: 0x2545_f491_4f6c_dd1d,
        }
    }
}
impl Jitter {
    /// xorshift64*: равномерное `[0.1, 0.9]`.
    fn next(&mut self) -> f64 {
        let mut x = self.seed;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.seed = x;
        let unit = (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64;
        0.1 + unit * 0.8
    }
}

/// Порт `parse_views`: включено в конфиге + тянет описание.
pub async fn parse_views(
    client: &Arc<AvitoClient>,
    ads: &mut [Listing],
    parse_views: bool,
    parse_description: bool,
    concurrency: usize,
) -> Result<usize> {
    if !parse_views {
        return Ok(0);
    }
    tracing::info!("parsing views for {} listings", ads.len());
    enrich(client, ads, true, parse_description, concurrency).await
}

/// Порт `parse_description`: только когда `parse_views` выключен.
pub async fn parse_description(
    client: &Arc<AvitoClient>,
    ads: &mut [Listing],
    parse_views: bool,
    parse_description: bool,
    concurrency: usize,
) -> Result<usize> {
    if !parse_description || parse_views {
        return Ok(0);
    }
    tracing::info!("parsing full description for {} listings", ads.len());
    enrich(client, ads, false, true, concurrency).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_rs::json;
    #[test]
    fn nested_dict_degrades_to_empty() {
        let v = json!({"success": {}});
        assert!(nested(&v, &["success", "mobile"]).is_object());
        assert!(nested(&json!({}), &["a"]).is_object());
    }
    #[test]
    fn counts_ints_str_and_ignores_bools() {
        let v = json!({
            "a": 5,
            "b": "1 234",
            "c": true,
            "d": "нет"
        });
        assert_eq!(count(JsonValueTrait::get(&v, "a")), Some(5));
        assert_eq!(count(JsonValueTrait::get(&v, "b")), Some(1234));
        assert_eq!(count(JsonValueTrait::get(&v, "c")), None);
        assert_eq!(count(JsonValueTrait::get(&v, "d")), None);
        assert_eq!(count(None), None);
    }
    #[test]
    fn mobile_stats_take_priority() {
        let payload = json!({
            "success": {"mobile": {"stats": {"views": {
                "total": 10, "today": 2
            }}, "description": "  hello  "}}
        });
        let (desc, total, today) = extract_item_details(&payload);
        assert_eq!(desc.as_deref(), Some("hello"));
        assert_eq!(total, Some(10));
        assert_eq!(today, Some(2));
    }
    #[test]
    fn falls_back_to_card_views_and_segments() {
        let payload = json!({
            "success": {"view": {"scenario": {"beduin": {"main": {"params": {
                "metaDataAndStats": {"views": {
                    "total": "1 200", "todayViews": 7
                }},
                "description": {"segments": [
                    {"text": "раз"}, {"text": "два"}, {"nope": 1}
                ]}
            }}}}}}
        });
        let (desc, total, today) = extract_item_details(&payload);
        assert_eq!(desc.as_deref(), Some("раздва"));
        assert_eq!(total, Some(1200));
        assert_eq!(today, Some(7));
    }
    #[test]
    fn segment_separator_becomes_newline() {
        let payload = json!({
            "success": {"view": {"scenario": {"beduin": {"main": {"params": {
                "description": {"segments": [{"text": "a\u{2028}b"}]}
            }}}}}}
        });
        let (desc, _, _) = extract_item_details(&payload);
        assert_eq!(desc.as_deref(), Some("a\nb"));
    }
    #[test]
    fn jitter_stays_in_range() {
        let mut j = Jitter::default();
        for _ in 0..64 {
            let v = j.next();
            assert!((0.1..=0.9).contains(&v), "{v}");
        }
    }
}
