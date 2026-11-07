# Документация avito-rs

| Документ | Назначение |
|----------|------------|
| [config-reference.md](config-reference.md) | Полный список ключей TOML с дефолтами и типами |
| [adr/](adr/) | Архитектурные решения (формат Michael Nygard) |
| [../config.example.toml](../config.example.toml) | Рабочий стартовый конфиг (все ключи) |

| ADR | Решение |
|----|---------|
| [01](adr/01-blocking-url-converter.md) | Конвертер «заблокированных» URL |
| [02](adr/02-no-notif-sdks.md) | Без SDK для Telegram/VK |
| [03](adr/03-camelcase-item-fields.md) | camelCase в полях Авито |
| [04](adr/04-lib-bin-split.md) | Разделение lib/bin под тесты |
| [05](adr/05-api-base-override-for-testability.md) | `api_base_override` для e2e |
| [06](adr/06-rest-api-serve-subcommand.md) | Команда `serve` (feature `api`) |

## Связанные материалы

- `../README.md` — обзор, архитектура, быстрый старт.
- `../tests/` — golden-тесты (что проверяют: `tests/evals/main.rs`).
- `../evals/datasets/v1/` — фиксированные входы для golden-сверок.
