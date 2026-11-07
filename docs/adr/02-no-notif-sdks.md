# 02. Skip teloxide and vk-sdk crates

- **Status:** Accepted
- **Date:** 2026-11-05
- **Deciders:** @Githab-capibara
- **Related:** `src/providers/telegram.rs`, `src/providers/vk.rs`

## Context

Оригинал шлёт через `messages.send` (VK) и `sendMessage`/`sendPhoto` (TG).
Крейты `teloxide` и `vk-sdk` тянут макросы, long-polling, типы на 50+ методов;
мы используем 4 и 3 метода соответственно. Залёток типа «загрузить фото в
messages upload» и так нет в зрелых SDK.

## Decision

Реализуем HTTP-вызовы на `reqwest` напрямую: JSON body, multipart для фото VK,
два ретрая с exponential backoff.

## Consequences

- **Easier:** число зависимостей меньше; поведение полностью соответствует оригиналу.
- **Harder:** своя реализация backoff; своя типизация ошибок API.
- **Given up:** удобство SDK (конструкторы клавиатур и т.п.) — не требуется.
- **Migration:** замена бекенда — правка одного метода `send`/`notify`.

## Alternatives considered

- **Option A:** `teloxide` + `vk-sdk` — rejected: оверхед 20+ трансloadов, лишний API-слой.
- **Option B:** `reqwest` + своя типизация — accepted.
