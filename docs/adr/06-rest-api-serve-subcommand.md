# 06. Expose feature-gated REST API via `serve` subcommand

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** @Githab-capibara
- **Related:** `src/api/`, `src/main.rs`, `tests/integration.rs`

## Context

Модуль `src/api/` существовал с роутером, проверкой `X-Api-Key` и
эндпоинтами `/start`, `/stop`, `/urls`, `/config`, `/metrics`, `/status`, но
не был подключён к CLI — компилировался под `#[cfg(feature = "api")]` как
мёртвый код. Управлять монитором без перезапуска было нельзя.

## Decision

CLI получает подкоманду `serve --config <путь> --bind host:port`, доступную
только с `--features api`. Она поднимает `ApiState` с живым конфигом и
счётчиками, публикует роутер и обслуживает его с graceful-shutdown по
Ctrl+C. Цикл монитора `serve` **не** запускает — `/start` и `/stop`
переключают состояние и шлют сигнал через `watch`-канал, как было
спроектировано изначально.

## Consequences

- **Easier:** монитор можно перенастроить (`PUT /urls`, `GET /config`) и
  наблюдать (`/metrics`, `/status`) без остановки процесса;
- **Harder:** по-прежнему нет проводки сигнала `watch` в настоящий `run` —
  управление состоянием остаётся заглушкой;
- **Given up:** аутентификация по одному статическому ключу из конфига;
  ротация и ACL не реализованы;
- **Migration:** ничего, `serve` — новая команда, старые пути не тронуты.

## Alternatives considered

- **Option A:** поднимать API автоматически внутри `run` — отклонено:
  монитор и HTTP-слой получают разные жизненные циклы и разные требования к
  правам;
- **Option B:** unix-socket + file-IPC — отклонено: требует утилиты
  клиента, тогда как `/status` надёжно проверяется curl;
- **Option C:** отдельный бинарник `avito-rs-api` — отклонено: дублирование
  загрузчика конфига и версии.
