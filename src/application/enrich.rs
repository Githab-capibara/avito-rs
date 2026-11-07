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

use std::time::Duration;

use anyhow::Result;

use crate::dto::Listing;
use crate::providers::avito::AvitoClient;

/// Порт `_extract_item_details`: (описание, всего, сегодня).
#[must_use]
pub fn extract_item_details(payload: &serde_json::Value) -> (Option<String>, Option<u64>, Option<u64>) {
    let mobile = nested(payload, &["success", "mobile"]);
    let mobile_views = nested(&mobile, &["stats", "views"]);
    let card_params = nested(
        payload,
        &["success", "view", "scenario", "beduin", "main", "params"],
    );
    let card_views = nested(&card_params, &["metaDataAndStats", "views"]);

    let total = count(mobile_views.get("total"))
        .or_else(|| count(card_views.get("total")));
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
fn card_description(card_params: &serde_json::Value) -> Option<String> {
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
fn nested(value: &serde_json::Value, keys: &[&str]) -> serde_json::Value {
    let mut current = value;
    for key in keys {
        match current.get(key) {
            Some(next) => current = next,
            None => return serde_json::Value::Object(Default::default()),
        }
    }
    current.clone()
}

/// Порт `count`: `bool` игнорируется, `int` берётся как есть, строка —
/// только цифры.
fn count(value: Option<&serde_json::Value>) -> Option<u64> {
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

/// Порт `_enrich_ads`: обойти список, скачать карточки, заполнить поля.
///
/// Флаги `views`/`description` управляют тем, что именно писать;
/// в оригинале при `parse_views=true` описание тоже подтягивается,
/// а `parse_description` работает только когда `parse_views` выключен.
pub async fn enrich(
    client: &AvitoClient,
    ads: &mut [Listing],
    views: bool,
    description: bool,
) -> Result<usize> {
    let total = ads.len();
    let mut enriched = 0usize;
    let mut rng = Jitter::default();
    for (index, ad) in ads.iter_mut().enumerate() {
        let id = match ad.id.parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                tracing::warn!("skipping listing without numeric id: {}", ad.id);
                continue;
            }
        };
        let page = match client.fetch_item(&id.to_string()).await {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!("card request failed for id {id}: {err}");
                continue;
            }
        };
        let Some(payload) = page.body.as_ref() else {
            tracing::warn!("card payload for id {id} is not JSON");
            continue;
        };
        let (full_description, total_views, today_views) =
            extract_item_details(payload);
        if views {
            ad.total_views = total_views;
            ad.today_views = today_views;
            tracing::debug!(
                "id {id} ({}/{total}) views total={total_views:?} today={today_views:?}",
                index + 1,
            );
        }
        if description {
            if let Some(text) = full_description {
                ad.description = Some(text);
            }
        }
        enriched += 1;
        tokio::time::sleep(Duration::from_secs_f64(rng.next())).await;
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
    client: &AvitoClient,
    ads: &mut [Listing],
    parse_views: bool,
    parse_description: bool,
) -> Result<usize> {
    if !parse_views {
        return Ok(0);
    }
    tracing::info!("parsing views for {} listings", ads.len());
    enrich(client, ads, true, parse_description).await
}

/// Порт `parse_description`: только когда `parse_views` выключен.
pub async fn parse_description(
    client: &AvitoClient,
    ads: &mut [Listing],
    parse_views: bool,
    parse_description: bool,
) -> Result<usize> {
    if !parse_description || parse_views {
        return Ok(0);
    }
    tracing::info!("parsing full description for {} listings", ads.len());
    enrich(client, ads, false, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_dict_degrades_to_empty() {
        let v = serde_json::json!({"success": {}});
        assert!(nested(&v, &["success", "mobile"]).is_object());
        assert!(nested(&serde_json::json!({}), &["a"]).is_object());
    }
    #[test]
    fn counts_ints_str_and_ignores_bools() {
        let v = serde_json::json!({
            "a": 5,
            "b": "1 234",
            "c": true,
            "d": "нет"
        });
        assert_eq!(count(v.get("a")), Some(5));
        assert_eq!(count(v.get("b")), Some(1234));
        assert_eq!(count(v.get("c")), None);
        assert_eq!(count(v.get("d")), None);
        assert_eq!(count(None), None);
    }
    #[test]
    fn mobile_stats_take_priority() {
        let payload = serde_json::json!({
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
        let payload = serde_json::json!({
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
        let payload = serde_json::json!({
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
