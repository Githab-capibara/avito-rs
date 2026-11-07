//! Регенерация JSON-датасетов `evals/datasets/v1/*.json` из Rust-фикстур.
//!
//! Даты-тесты (`datasets`) читают файлы, поэтому они должны лежать в git.
//! Обновить: `cargo test --test gen_datasets -- --ignored`.

use std::path::Path;

#[path = "evals/fixtures.rs"]
#[allow(dead_code)]
mod fixtures;

#[test]
#[ignore = "run manually: cargo test --test gen_datasets -- --ignored"]
fn regenerate_datasets() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/datasets/v1");
    std::fs::create_dir_all(&dir).unwrap();

    let page = fixtures::raw_page();
    let expected: Vec<serde_json::Value> = fixtures::EXPECTED
        .iter()
        .map(|(id, price, title)| {
            serde_json::json!({ "id": id, "price": price, "title": title })
        })
        .collect();

    write(&dir, "catalog_two_ads.json", &serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&page).unwrap()).unwrap());
    write(&dir, "catalog_price_drop.json", &serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&fixtures::raw_page_cheap_only()).unwrap()).unwrap());
    write(&dir, "expected_listings.json", &serde_json::to_string_pretty(&expected).unwrap());
}

/// Записать файл с завершающим переводом строки.
fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), format!("{body}\n")).unwrap();
}
