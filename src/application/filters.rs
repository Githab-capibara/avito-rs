//! Конвейер фильтров — порт `filters/ads_filter.py` (9 фильтров).
//!
//! Каждый фильтр — чистая функция `&[Listing] -> Vec<Listing>`; порядок и
//! правила раннего выхода повторяют оригинал. Фильтр, который ничего не
//! меняет (пустой список слов/лимитов), стоит дешевле проверки входа.

use crate::dto::{FilterCfg, Listing};

/// Пробег по всем фильтрам (порядок как в оригинальном `AdsFilter.apply`).
///
/// Цепочка: цена → чёрные слова → белые слова → адрес → продавец → возраст →
/// бронь → продвижение. Каждый фильтр — чистая функция; пустые условия
/// пропускаются дёшево (`return ads`).
#[must_use]
pub fn apply(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    let mut ads = by_price_range(cfg, ads);
    ads = by_keywords_blacklist(cfg, ads);
    ads = by_keywords_whitelist(cfg, ads);
    ads = by_region(cfg, ads);
    ads = by_seller_blacklist(cfg, ads);
    ads = by_max_age(cfg, ads);
    ads = by_reserved(cfg, ads);
    by_promotion(cfg, ads)
}

/// Чёрный список слов: убрать, если слово встречается в `title + description`.
fn by_keywords_blacklist(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    if cfg.keywords_black.is_empty() {
        return ads;
    }
    let words: Vec<String> = cfg
        .keywords_black
        .iter()
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return ads;
    }
    ads.into_iter()
        .filter(|ad| {
            let text = ad.search_text();
            !words.iter().any(|w| text.contains(w.as_str()))
        })
        .collect()
}

/// Белый список: оставить только если хоть одно слово встречается.
fn by_keywords_whitelist(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    if cfg.keywords_white.is_empty() {
        return ads;
    }
    let words: Vec<String> = cfg
        .keywords_white
        .iter()
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return ads;
    }
    ads.into_iter()
        .filter(|ad| {
            let text = ad.search_text();
            words.iter().any(|w| text.contains(w.as_str()))
        })
        .collect()
}

/// Фильтр по адресу: подстрока в `geo.formattedAddress` (без учёта регистра).
fn by_region(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    let Some(geo) = cfg.region.as_ref().map(|g| g.trim().to_lowercase()) else {
        return ads;
    };
    if geo.is_empty() {
        return ads;
    }
    ads.into_iter()
        .filter(|ad| {
            ad.formatted_address
                .as_deref()
                .is_some_and(|a| a.to_lowercase().contains(&geo))
        })
        .collect()
}

/// Чёрный список продавцов.
///
/// В оригинале сравнение точное: `ad.sellerId not in
/// config.seller_black_list`. Объявления без `sellerId` проходят (как и там).
fn by_seller_blacklist(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    if cfg.sellers_blacklist.is_empty() {
        return ads;
    }
    let bad: Vec<String> = cfg
        .sellers_blacklist
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if bad.is_empty() {
        return ads;
    }
    ads.into_iter()
        .filter(|ad| {
            ad.seller_id
                .as_deref()
                .is_none_or(|s| !bad.iter().any(|b| s == b))
        })
        .collect()
}

/// Диапазон цен (порт `_filter_by_price_range`).
///
/// Особенности оригинала:
/// - `max_price = 0` (или `None`) трактуется как `+∞` (`float("inf")`);
/// - объявление **без цены** приравнивается к `0`, поэтому падает под
///   `min_price > 0` (в оригинале `val = ... else 0`);
/// - если `min_price == 0` и предела сверху нет — фильтр пропускается.
fn by_price_range(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    let min = cfg.min_price;
    let max = cfg.max_price.filter(|m| *m > 0).unwrap_or(u64::MAX);
    if min == 0 && max == u64::MAX {
        return ads;
    }
    ads.into_iter()
        .filter(|ad| {
            let value = ad.price.unwrap_or(0);
            value >= min && value <= max
        })
        .collect()
}

/// Убрать забронированные (порт `_filter_by_reserve`).
fn by_reserved(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    if !cfg.ignore_reserved {
        return ads;
    }
    ads.into_iter().filter(|ad| !ad.is_reserved).collect()
}

/// Убрать продвигаемые (порт `_filter_by_promotion`).
fn by_promotion(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    if !cfg.ignore_promotion {
        return ads;
    }
    ads.into_iter().filter(|ad| !ad.is_promotion).collect()
}

/// Ограничение возраста: `sortTimeStamp` (мс) не старше `max_age_sec`.
///
/// Порт `_filter_by_max_age`: объявления без метки времени отбрасываются,
/// как и в оригинале (`if not ad.sortTimeStamp: continue`).
fn by_max_age(cfg: &FilterCfg, ads: Vec<Listing>) -> Vec<Listing> {
    let max_age = cfg.max_age_sec;
    if max_age == 0 {
        return ads;
    }
    let now_ms = now_millis();
    ads.into_iter()
        .filter(|ad| {
            let Some(stamp) = ad.sort_time_stamp else {
                return false;
            };
            let age_ms = now_ms.saturating_sub(i128::from(stamp));
            age_ms <= i128::from(max_age) * 1000
        })
        .collect()
}

/// Текущее Unix-время в миллисекундах.
fn now_millis() -> i128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i128::from(d.as_secs()))
        .unwrap_or(0)
        * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ad(id: &str, title: &str, desc: &str, price: u64) -> Listing {
        Listing {
            id: id.to_owned(),
            title: Some(title.to_owned()),
            description: Some(desc.to_owned()),
            price: Some(price),
            ..Listing::default()
        }
    }

    fn now_ms() -> i128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        i128::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(0))
                .unwrap_or(0),
        )
    }

    #[test]
    fn empty_config_passes_everything() {
        let ads = vec![ad("1", "x", "y", 10)];
        assert_eq!(apply(&FilterCfg::allow_all(), ads).len(), 1);
    }

    #[test]
    fn blacklist_keyword_excludes() {
        let cfg = FilterCfg {
            keywords_black: vec!["бу".to_owned()],
            ..FilterCfg::allow_all()
        };
        let ads = vec![ad("1", "Бульдозер", "", 100), ad("2", "телефон", "", 100)];
        let out = apply(&cfg, ads);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "2");
        assert_eq!(out[0].price, Some(100));
    }

    #[test]
    fn whitelist_requires_match() {
        let cfg = FilterCfg {
            keywords_white: vec!["iphone".to_owned()],
            ..FilterCfg::allow_all()
        };
        let ads = vec![ad("1", "iPhone 16", "", 10), ad("2", "Samsung", "", 10)];
        assert_eq!(apply(&cfg, ads).len(), 1);
    }

    #[test]
    fn region_matches_address() {
        let cfg = FilterCfg {
            region: Some("краснодар".to_owned()),
            ..FilterCfg::allow_all()
        };
        let mut a = ad("1", "t", "", 1);
        a.formatted_address = Some("Краснодар, ул. Красная".to_owned());
        let ads = vec![a, ad("2", "t", "", 1)];
        assert_eq!(apply(&cfg, ads).len(), 1);
    }

    #[test]
    fn max_and_min_price() {
        let cfg = FilterCfg {
            max_price: Some(100),
            min_price: 50,
            ..FilterCfg::allow_all()
        };
        let ads = vec![ad("1", "t", "", 10), ad("2", "t", "", 75), ad("3", "t", "", 500)];
        let out = apply(&cfg, ads);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "2");
    }

    #[test]
    fn max_age_drops_stale_and_unknown() {
        let cfg = FilterCfg {
            max_age_sec: 3600,
            ..FilterCfg::allow_all()
        };
        let mut fresh = ad("1", "t", "", 1);
        fresh.sort_time_stamp = Some(now_ms() as i64 - 1000);
        let mut stale = ad("2", "t", "", 1);
        stale.sort_time_stamp = Some(now_ms() as i64 - 10_000_000);
        let out = apply(&cfg, vec![fresh, stale, ad("3", "t", "", 1)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "1");
    }

    #[test]
    fn seller_blacklist_exact() {
        let cfg = FilterCfg {
            sellers_blacklist: vec!["spammer".to_owned()],
            ..FilterCfg::allow_all()
        };
        let mut a = ad("1", "t", "", 1);
        a.seller_id = Some("spammer".to_owned());
        let mut b = ad("2", "t", "", 1);
        b.seller_id = Some("goodshop".to_owned());
        let out = apply(&cfg, vec![a, b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].seller_id.as_deref(), Some("goodshop"));
    }
}
