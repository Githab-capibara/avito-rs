//! Golden-эвалы поверх файлов `evals/datasets/v1/*.json`.
//!
//! Наборы — «замороженные» входы/выходы оригинального `parser_avito`:
//! валидный ответ mobile API со всеми полями, которые реально используются.

use std::path::Path;

use avito_rs::extractors::{active, ExtractError, ExtractorKind};
use avito_rs::providers::FetchedPage;

/// Прочитать JSON-датасет и обернуть в `FetchedPage`.
fn load(name: &str) -> FetchedPage {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("evals/datasets/v1")
        .join(name);
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    FetchedPage {
        url: "https://www.avito.ru/all/telefony".to_owned(),
        status: 200,
        body: sonic_rs::from_str(&raw).ok(),
        raw_body: raw.into_bytes(),
    }
}

#[tokio::test]
async fn dataset_catalog_two_ads_matches_expected() {
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/datasets/v1/expected_listings.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let page = load("catalog_two_ads.json");
    let ads = active(ExtractorKind::V1Json)
        .extract(&page)
        .expect("catalog must extract");
    let got: Vec<serde_json::Value> = ads
        .iter()
        .map(|a| serde_json::json!({ "id": a.id, "price": a.price, "title": a.title }))
        .collect();
    assert_eq!(
        serde_json::json!(&got),
        expected,
        "golden listing set drifted"
    );
}

#[tokio::test]
async fn dataset_price_drop_is_single_fresh_ad() {
    let page = load("catalog_price_drop.json");
    let ads = active(ExtractorKind::V1Json).extract(&page).unwrap();
    assert_eq!(ads.len(), 1);
    assert_eq!(
        ads[0].id, "5050505050",
        "price drop keeps the cheap listing"
    );
    assert_eq!(ads[0].price, Some(49_999));
}

#[test]
fn dataset_unknown_shape_fails_typed() {
    // Датасет с неожиданной формой (нет items) — ошибка формы NoItems.
    let bad = r#"{"unexpected": {"nested": true}}"#;
    let page = FetchedPage {
        url: "fixture".to_owned(),
        status: 200,
        body: sonic_rs::from_str(bad).ok(),
        raw_body: bad.as_bytes().to_vec(),
    };
    assert!(matches!(
        active(ExtractorKind::V1Json).extract(&page),
        Err(ExtractError::NoItems { .. })
    ));
}
