# 08. Migrate the runtime HTTP stack to wreq

- **Status:** Accepted
- **Date:** 2026-11-10
- **Deciders:** @Githab-capibara
- **Related:** [README.md](../../README.md)

## Context

Avito fingerprints TLS and throttles/kills requests from standard clients.
Everything we send today carries `reqwest`'s default JA3, which the site already
recognises; the whole point of the port is to survive that.

`wreq` 0.16 (`0x676e67/wreq`) is a `reqwest`-shaped client on BoringSSL with
browser-parity JA3/JA4 and HTTP/2 signatures, plus the same `Client`,
`RequestBuilder`, `Proxy`, `multipart`, `.json()`, `.form()`, `cookie_store`
surface we already use.

Constraint found during research: `wreq` is **async-only** — there is no
`blocking` feature (`cargo metadata` confirms). So the sync call site must move
to async, or wrap in a runtime handle.

Second constraint: `wreq::Client` does not implement `Debug`, while
`TelegramNotifier` / `VkNotifier` used `#[derive(Debug)]` on it.

## Decision

Move all runtime HTTP (`src/providers/{avito,telegram,vk}.rs`) to `wreq`, keeping
`reqwest` as a **dev-dependency only** (`blocking` for `tests/integration.rs`).

Concretely:

- `Cargo.toml`: `reqwest` reduced to `blocking`; `wreq` added with
  `webpki-roots, tokio-rt, charset, cookies, gzip, json, multipart, form, query,
  socks, stream`.
- `convert_url` became `async` and dropped `tokio::task::spawn_blocking`: a
  120 s third-party call was pinning a blocking worker. With async it now blocks
  nothing, and the timeout stays on the call.
- `TelegramNotifier` / `VkNotifier`: `#[derive(Debug)]` removed (no consumer of
  that `Debug` exists in the crate; nothing printed them).

## Consequences

- **Easier:** requests now present a browser TLS/HTTP2 fingerprint instead of
  `reqwest`'s, which is the actual defence against the blocks we already rotate
  proxies for.
- **Easier:** one fewer blocking-worker pinned during a URL conversion.
- **Harder:** build now needs a C toolchain (`cmake`, `perl`, `clang`) because
  `btls-sys` vendors BoringSSL. Release/first-install time jumps to ~1 min.
- **Harder:** `wreq` API is mirror-shaped but not identical for new features —
  e.g. no `blocking`, no `Debug` on `Client`.
- **Given up:** `rustls-tls` is gone from the runtime path; TLS roots now come
  from `webpki-roots`.
- **Migration:** source change only, tests unchanged (they use dev-dep
  `reqwest::blocking`). Verified: 70 tests (incl. e2e against mock), clippy
  `-D warnings` with and without features, `cargo fmt --check`.

## Alternatives considered

- **Option A: keep `reqwest` and set a fake JA3 via headers** — rejected:
  headers cannot forge the TLS ClientHello, which is where Avito reads the
  fingerprint.
- **Option B: `rquest`** — rejected: all published versions are yanked on
  crates.io.
- **Option C: `impit`** — rejected: 0.1.0, stale, no browser impersonation.
- **Option D: wrap `convert_url` in `spawn_blocking` + `Handle::current().block_on`**
  — rejected: keeps a worker pinned for the full 120 s timeout; async version is
  strictly better now that `wreq` is async-only.
