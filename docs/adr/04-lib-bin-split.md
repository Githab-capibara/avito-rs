# 04. Split crate into lib and bin

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** @Githab-capibara
- **Related:** `src/lib.rs`, `src/main.rs`, `tests/`

## Context

Интеграционные golden-тесты должны дёргать те же extractors/storage, что и
CLI. При одном бинарнике `main.rs` это невозможно без дублирования модулей.
К тому же в `main.rs` был мёртвый `mod api` без `#[cfg(feature)]`.

## Decision

Логика — в `src/lib.rs` (все модули), `src/main.rs` — тонкий CLI поверх
`avito_rs`. Тесты лежат в `tests/`, datasets в `evals/datasets/v1/`.

## Consequences

- **Easier:** тесты видят внутренности; CLI остаётся простым.
- **Harder:** двойной набор `pub`-пометок (либа должна экспортировать модули наружу).
- **Given up:** ничего.
- **Migration:** `cargo build` копится быстрее (lib кэшируется отдельно от bin).

## Alternatives considered

- **Option A:** всё в `main.rs` + unit-тесты inline — rejected: интеграционные тесты невозможны.
- **Option B:** lib + bin — accepted.
