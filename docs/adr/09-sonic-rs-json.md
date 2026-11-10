# 09. Migrate JSON parsing to sonic-rs

- **Status:** Accepted
- **Date:** 2026-11-11
- **Deciders:** @Githab-capibara
- **Related:** [config-reference.md](../config-reference.md)

## Context

`serde_json` is the correct default and we keep it for tests/fixtures and for
`axum`'s `Json` extractor (which is hard-wired to `serde_json::Value`). But on
the hot path — a 50 KB catalog page parsed once per source per cycle — it is the
single biggest non-HTTP cost in the monitor loop.

`sonic-rs` 0.5 (cloudwego) is a SIMD serde_json-compatible parser using
pre-allocated buffers, with `from_str`, `to_string`, `Value`, `from_value`,
`to_value`, `json!`, `Object`, `Array`.

## Decision

Move the runtime JSON path (`FetchedPage::body`, `Item`, `find_items_node`,
`v2_html::state`, `enrich`, `xlsx` sidecar, `providers::from_json`) to
`sonic_rs::Value`, and keep `serde_json` as a dev/structure dependency only.

Two API gaps found and worked around, both recorded here so the next reader
does not rediscover them:

1. `sonic_rs::Value` is an **opaque struct with traits**, not an enum.
   `match v { Value::Null | Value::String(..) }` does not compile; there is no
   `Value::Object` constructor either. Access is `v.get_type()` +
   `as_*`/`is_*` from `JsonValueTrait`, and `v.as_object()?.get(&"key")` from
   `JsonContainerTrait` (`Object::get<Q: AsRef<str>>(&Q)` — note the extra
   reference; plain `obj.get("k")` fails to compile).
   `Object` has `iter()` but no `values()`.
2. `sonic_rs::from_value` **cannot deserialize into a field typed
   `sonic_rs::Value`** — it errors with `invalid type: newtype struct, expected
   a valid json`. Verified with a standalone repro (it works for `Option<String>`
   fields, fails for any `Option<sonic_rs::Value>`).
   `extract_items` therefore round-trips: `to_string(node)` → `from_str`.
   Costs one extra serialize pass per page; at ~50 KB/page that is still far
   below the HTTP request that produced the page.

## Consequences

- **Easier:** hot-path parse is faster (SIMD, buffer-reusing).
- **Easier:** `serde_json` stays where it is genuinely needed (axum `Json`,
  test fixtures), so the dependency is not deleted, only demoted.
- **Harder:** `Value` is trait-based, so every match on `Value` variants and
  every `.get("k")` call had to be rewritten; `JsonValueTrait` must be in scope
  per module.
- **Harder:** no `pointer()` method — `serde_json`'s `"/a/b"` path strings become
  `sonic_rs::pointer!["a", "b"]` with `JsonValueTrait::pointer`.
- **Given up:** `from_value` (zero-copy) on the catalog path, in exchange for
  `from_str` on the serialized node.
- **Migration:** source change only. Verified 62 tests (default features) and 76
  with `api,xlsx,html-fallback`, `cargo clippy --all-targets -- -D warnings`
  for both sets, `cargo fmt --check`.

## Alternatives considered

- **Option A: `serde_json` untouched, optimize elsewhere** — rejected: the parse
  is the dominant non-HTTP cost and no other change targets it.
- **Option B: drop `Item::id/iva/images/coords` as `Value` and type them
  strictly** — rejected for this pass: it changes public API shape and the
  golden datasets encode the loose behaviour. Revisit as a separate ADR.
- **Option C: use `LazyValue` to avoid the round-trip in `extract_items`** —
  deferred: it would avoid the `to_string`, but requires reworking
  `find_items_node` to return borrowed views. Recorded as a follow-up.
