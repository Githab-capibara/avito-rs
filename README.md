# avito-rs — Rust-порт parser_avito

[![crates.io](https://img.shields.io/badge/crates.io-pending-orange.svg)]()
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.98%2B-blue.svg)](https://www.rust-lang.org)

> Монитор объявлений Авито: фильтры, дедупликация по цене, уведомления в
> Telegram/VK, экспорт CSV/XLSX. Порт Python-оригинала
> [`Duff89/parser_avito`](https://github.com/Duff89/parser_avito).

## Архитектура

```text
src/
├── lib.rs              публичный API (все модули)
├── main.rs             CLI: run / status / validate-config
├── dto.rs              конфиг + DTO (urls, filters, notifications, export)
├── providers/          сеть: avito.rs, telegram.rs, vk.rs, backoff.rs
├── extractors/         вытаскивание Listing из ответа API (v1_json, v2_html)
├── application/        фильтры + цикл монитора (Monitor)
├── storage/            SQLite (rusqlite): dedup по (id, price)
├── exporters/          CSV (feature xlsx — XLSX, те же 14 колонок оригинала)
├── application/enrich.rs  обогащение: просмотры/описание из карточек
├── security/           Redactor: секреты не попадают в логи
├── observability/      tracing + метрики
└── api/                REST API (feature api, axum)

tests/                  интеграционные golden-тесты
evals/datasets/v1/      фиксированные входы/выходы для golden-сверок
```

Поток данных: `config.toml` → `AppConfig` → `Monitor::run_once` →
`AvitoClient::fetch` → `Extractor::extract` → `filters::apply` → dedup
(`SqliteStore`) → `CsvExporter` → (опционально) Telegram/VK.

## Быстрый старт

```bash
cargo build --release

cat > config.toml <<'TOMl'
interval_sec = 60
urls = ["https://www.avito.ru/krasnodar/telefony"]
db_path = "./database.db"

[filters]
max_price = 50000
region = "Краснодар"
keywords_black = ["ремонт", "на запчасти"]

[notifications]
telegram_bot_token = "123456:XXX"
telegram_chat_ids = [123456789]

[export]
dir = "./out"
format = "csv"
TOML

./target/release/avito-rs validate-config --config config.toml
./target/release/avito-rs run --config config.toml --once   # одиночный проход
./target/release/avito-rs run --config config.toml          # вечный цикл, Ctrl+C = стоп
./target/release/avito-rs status --config config.toml        # состояние БД
./target/release/avito-rs serve --config config.toml --bind 127.0.0.1:8080   # REST API (нужен --features api)
```

## Ключевые решения (ADR)

| Решение | Почему | Где |
|---------|--------|-----|
| `reqwest::blocking` для конвертера URL | Внешний сервис медленный (до 60с); вызов один раз, дальше кэш на диске | [01](./docs/adr/01-blocking-url-converter.md) |
| Нет `teloxide` / `vk-sdk` | 4 метода Bot API и 3 метода VK API проще держать в reqwest | [02](./docs/adr/02-no-notif-sdks.md) |
| `serde(rename_all = "camelCase")` на `Item` | Иначе `priceDetailed`/`urlPath` молча игнорировались | [03](./docs/adr/03-camelcase-item-fields.md) |
| lib + bin раздельно | интеграционные тесты (`tests/`) требуют публичного API | [04](./docs/adr/04-lib-bin-split.md) |
| `api_base_override` | e2e-прогон против мока Авито без cookies/прокси | [05](./docs/adr/05-api-base-override-for-testability.md) |
| Команда `serve` | модуль `src/api/` был мёртвым кодом без CLI-входа | [06](./docs/adr/06-rest-api-serve-subcommand.md) |

## Feature flags

| Флаг | Включает | По умолчанию |
|------|----------|--------------|
| `api` | REST API (`axum`) + команда `serve` | выкл |
| `xlsx` | экспорт в `.xlsx` (`rust_xlsxwriter`) | выкл |
| `html-fallback` | v2-экстрактор через `<script data-mfe-state>` (`scraper`) | выкл |

## Tests

```bash
cargo test                 # 43 unit + golden-теста
cargo test --test evals    # извлечение, dedup, фильтры
cargo test --test datasets # golden-сверка с evals/datasets/v1/*.json
cargo test --test gen_datasets -- --ignored  # обновить датасеты
```
