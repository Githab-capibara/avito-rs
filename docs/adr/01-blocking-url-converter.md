# 01. Use blocking reqwest for url converter

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** @Githab-capibara
- **Related:** `src/providers/avito.rs::convert_url`

## Context

Режим `fetch_mode = "spfa_url"` превращает публичный URL каталога
(`avito.ru/...`) в URL mobile API через сторонний сервис. Тот отвечает медленно
(документация оригинала допускает до минуты). Converter спам был бы прогонён в
`async` на каждый URL цикла; кэш на диске делает его одноразовой операцией,
но первый запуск всё равно может заблокировать event loop.

## Decision

Зовём converter через `reqwest::blocking::Client` (120s timeout) из синхронной
функции, затем кэшируем результат на диск.

## Consequences

- **Easier:** нет async-мусора вокруг одноразового вызова; event loop не блокируется.
- **Harder:** `blocking` фича растягивает граф зависимостей.
- **Given up:** возможность прогресса нескольких URL параллельно (кэш нивелирует).
- **Migration:** фича `blocking` в `reqwest` — назад несовместимо (но и не нужно).

## Alternatives considered

- **Option A:** `async` + таймаут в tokio — rejected: избыточный запас, кэш и так делает вызов редким.
- **Option B:** вынести converter в отдельный поток — rejected: `blocking` проще.
