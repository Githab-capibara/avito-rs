//! `avito-rs` — разбор, фильтрация и экспорт объявлений Авито.
//!
//! Библиотека отделена от бинаря, чтобы интеграционные тесты (`tests/`)
//! и golden-эвалы (`evals/`) могли пользоваться теми же модулями,
//! что и CLI.

pub mod api;
pub mod application;
pub mod dto;
pub mod exporters;
pub mod extractors;
pub mod observability;
pub mod providers;
pub mod security;
pub mod storage;
