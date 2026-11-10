//! Golden-эвалы экстрактора: фиксированные входы → фиксированные выходы.
//!
//! Сейчас фикстуры лежат inline (см. `fixtures`); при появлении
//! `evals/datasets/v1/*.json` тесты читают оттуда. Запуск: `cargo test --test evals`.

mod fixtures;

use avito_rs::extractors::{active, ExtractError, ExtractorKind};
use avito_rs::providers::FetchedPage;

/// Собрать `FetchedPage` из JSON-строки.
fn page_from_json(raw: &str) -> FetchedPage {
    FetchedPage {
        url: "https://www.avito.ru/test".to_owned(),
        status: 200,
        body: sonic_rs::from_str(raw).ok(),
        raw_body: raw.as_bytes().to_vec(),
    }
}

#[tokio::test]
async fn extracts_titles_prices_and_ids() {
    let fetcher = fixtures::FixtureFetcher::new();
    let page = fetcher
        .fetch("https://www.avito.ru/all/telefony")
        .await
        .unwrap();
    let extractor = active(ExtractorKind::V1Json);
    let ads = extractor.extract(&page).unwrap();
    assert_eq!(
        ads.len(),
        fixtures::EXPECTED.len(),
        "fixture items preserved"
    );
    for (ad, want) in ads.iter().zip(fixtures::EXPECTED.iter()) {
        assert_eq!(ad.id, want.0);
        assert_eq!(ad.price, want.1);
        assert_eq!(ad.title.as_deref(), Some(want.2));
    }
}

#[tokio::test]
async fn price_drop_produces_new_listing() {
    // Дедуп в БД по (id, price): тот же id с меньшей ценой — новая запись
    let fetcher = fixtures::FixtureFetcher::new();
    let page = fetcher
        .fetch("https://www.avito.ru/all/telefony?price_drop=1")
        .await
        .unwrap();
    let ads = active(ExtractorKind::V1Json).extract(&page).unwrap();
    let seen = avito_rs::storage::SqliteStore::open_memory().unwrap();
    let mut fresh = 0;
    for ad in &ads {
        if !seen.is_seen(ad).unwrap() {
            fresh += 1;
            seen.mark_seen(ad).unwrap();
        }
    }
    assert!(fresh > 0, "first pass must be fresh");
    let second = ads.iter().filter(|ad| !seen.is_seen(ad).unwrap()).count();
    assert_eq!(second, 0, "second pass fully deduped");
}

#[tokio::test]
async fn items_of_wrong_type_is_no_items() {
    // items не массив → find_items_node() его отфильтровывает, форма неожиданная.
    let page = page_from_json(r#"{"items": "not-an-array"}"#);
    let err = active(ExtractorKind::V1Json).extract(&page).unwrap_err();
    assert!(matches!(err, ExtractError::NoItems { .. }), "got {err:?}");
}

#[test]
fn missing_body_is_no_json() {
    let page = FetchedPage {
        url: "https://www.avito.ru/x".to_owned(),
        status: 200,
        body: None,
        raw_body: b"<html>captcha</html>".to_vec(),
    };
    let err = active(ExtractorKind::V1Json).extract(&page).unwrap_err();
    assert!(matches!(err, ExtractError::NoJson), "got {err:?}");
}

#[tokio::test]
async fn empty_items_yields_empty_vec() {
    // Пустой каталог — не ошибка: просто нет объявлений.
    let page = page_from_json(r#"{"items": []}"#);
    let ads = active(ExtractorKind::V1Json).extract(&page).unwrap();
    assert!(ads.is_empty(), "empty items -> empty vec");
}

#[tokio::test]
async fn no_items_key_is_no_items() {
    // Валидный JSON без ключа items — ошибка формы.
    let page = page_from_json(r#"{"result": {"somethingElse": true}}"#);
    let err = active(ExtractorKind::V1Json).extract(&page).unwrap_err();
    assert!(matches!(err, ExtractError::NoItems { .. }), "got {err:?}");
}

#[tokio::test]
async fn html_payload_without_json_is_no_json() {
    let page = FetchedPage {
        url: "https://www.avito.ru/x".to_owned(),
        status: 200,
        body: None,
        raw_body: b"<html><body>blocked</body></html>".to_vec(),
    };
    let err = active(ExtractorKind::V1Json).extract(&page).unwrap_err();
    assert!(matches!(err, ExtractError::NoJson), "got {err:?}");
}

#[tokio::test]
async fn v2_extractor_reports_no_json() {
    // v2 без feature html-fallback: на пустом HTML честно NoJson.
    let page = FetchedPage {
        url: "https://www.avito.ru/x".to_owned(),
        status: 200,
        body: None,
        raw_body: b"<html><body>no state script</body></html>".to_vec(),
    };
    let result = active(ExtractorKind::V2Html).extract(&page);
    assert!(
        matches!(result.unwrap_err(), ExtractError::NoJson),
        "v2 must fail no-json"
    );
}

#[tokio::test]
async fn filters_run_on_extracted_ads() {
    use avito_rs::application::filters::apply;
    use avito_rs::dto::{AppConfig, FilterCfg};
    let page = fixtures::FixtureFetcher::new()
        .fetch("https://www.avito.ru/all/telefony")
        .await
        .unwrap();
    let ads = active(ExtractorKind::V1Json).extract(&page).unwrap();
    let cfg = AppConfig {
        filters: FilterCfg {
            max_price: Some(50_000),
            ..FilterCfg::allow_all()
        },
        ..AppConfig::default()
    };
    let out = apply(&cfg.filters, ads);
    assert!(
        out.iter().all(|a| a.price.unwrap_or(0) <= 50_000),
        "max_price filter applied"
    );
}

#[tokio::test]
async fn v2_extractor_falls_back_or_extracts() {
    // v2 без feature html-fallback: NoJson, с feature — тоже NoJson на HTML без скрипта
    let page = FetchedPage {
        url: "https://www.avito.ru/x".to_owned(),
        status: 200,
        body: None,
        raw_body: b"<html><body>no state script</body></html>".to_vec(),
    };
    let result = active(ExtractorKind::V2Html).extract(&page);
    assert!(result.is_err(), "v2 on headerless html must fail cleanly");
    assert!(matches!(result.unwrap_err(), ExtractError::NoJson));
}

#[tokio::test]
async fn full_pipeline_from_fixture_to_csv() {
    use avito_rs::exporters::{csv::CsvExporter, ResultExporter};
    let ads = active(ExtractorKind::V1Json)
        .extract(&page_from_json(&fixtures::raw_page()))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let exp = CsvExporter::new(dir.path().join("out.csv"));
    exp.save(&ads).unwrap();
    let text = std::fs::read_to_string(dir.path().join("out.csv")).unwrap();
    assert_eq!(text.lines().count(), ads.len() + 1, "header + rows");
    for ad in &ads {
        assert!(text.contains(&ad.id));
    }
}
