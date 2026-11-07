# 03. Derive camelCase on extractor Item

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** @Githab-capibara
- **Related:** `src/extractors/v1_json.rs`

## Context

API mobile-каталога отдаёт ключи в camelCase (`priceDetailed`, `urlPath`,
`sortTimeStamp`, `isReserved`). Rust-структура объявляла поля в snake_case, а
serde без указания ищет идентификаторы как есть. Итог — поля молча
оставались `None`, цены не находились, фильтры по цене не работали.

## Decision

Проставляем `#[serde(rename_all = "camelCase")]` на `Item`, `PriceDetailed`,
`Geo`; фиксируем ожидания в `evals/datasets/v1/expected_listings.json`.

## Consequences

- **Easier:** golden-тест падает сразу при дрейфе полей.
- **Harder:** нет: rename — построчный маппинг, как был и в оригинале.
- **Given up:** ничего.
- **Migration:** тест `dataset_catalog_two_ads_matches_expected` покрывает.

## Alternatives considered

- **Option A:** ручные `#[serde(rename)]` на каждом поле — rejected: шумнее, легче ошибиться.
- **Option B:** парсить `serde_json::Value` вручную — rejected: теряем типизацию.
