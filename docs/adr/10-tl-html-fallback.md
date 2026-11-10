# 10. Parse the HTML fallback with tl instead of scraper

- **Status:** Accepted
- **Date:** 2026-11-11
- **Deciders:** @Githab-capibara
- **Related:** [08-wreq-migration.md](./08-wreq-migration.md)

## Context

`html-fallback` (`src/extractors/v2_html.rs`) is the direct-HTML mode: it finds
one `<script type="mime/invalid" data-mfe-state="true">` tag, decodes it, and
hands the JSON to the `v1_json` extractor.

Three facts made the current state wrong:

1. The feature was wired to the `scraper` crate, but **`scraper` was never
   used** — `grep -rn "scraper::" src/` is empty. The module used a hand-rolled
   linear string scanner (`find_state_script`) instead, so the feature flag
   pulled a full DOM crate (and `html5ever` transitively) for nothing.
2. That hand-rolled scanner had real bugs: `lower[start..].find('>')?` aborts
   the whole extractor with `?` on any `<script` without a closing `>`, and it
   returned the *first* matching script even though the loop kept scanning.
3. The fallback is exactly where a real browser-grade parser pays off, because
   the input there is a third-party HTML document (not our own API JSON).

`tl` 0.7 is a SIMD HTML parser with selector support
(`dom.query_selector("script[type][data-mfe-state]")`), `ParserOptions`, and
`HTMLTag::inner_text(parser) -> Cow<str>`.

## Decision

Replace the optional `scraper` dependency with `tl`, and implement
`find_state_script` via `tl::parse` + `query_selector`, filtering for
`type` containing `mime/invalid` and no `sandbox` attribute (the original
ignored sandboxed state scripts).

Behaviour that had to be preserved, and is now covered by a test
(`state_script_keeps_entities_and_escapes_verbatim`):

- `inner_text` on a `<script>` tag returns the **raw** text — Avito's payload
  contains HTML entities (`&amp;`) and JS escapes (`\u003c`) that must survive
  to `html_unescape`. Verified: `tl` does not decode entities inside `<script>`,
  so the existing `html_unescape` + `from_str` chain still works.

## Consequences

- **Easier:** `html-fallback` no longer pulls `scraper`/`html5ever` — the
  dependency graph shrinks by two crates and one full DOM implementation.
  `scraper` is gone from `Cargo.lock` (verified).
- **Easier:** the two scanner bugs are gone; missing `>` or a non-matching tag
  now just yields `None` → `ExtractError::NoJson`, which is the documented
  contract, instead of a `?` that aborted the caller.
- **Harder:** `find_state_script` now allocates a `String` rather than returning
  `&str` into the input (`inner_text` copies). Only one call site, on a cold path.
- **Harder:** `tl` is not spec-strict (its own README says so); malformed or
  extremely unusual markup can drop nodes. Acceptable here because we look for
  one well-known tag and fall back to `NoJson` otherwise.
- **Given up:** the "linear scan is cheaper" claim in the old comment. It was
  measured against a feature that never built a DOM at all; with a real parser
  the tradeoff is a one-pass SIMD parse over one `<script>` extraction.
- **Migration:** feature renamed internally (`dep:scraper` → `dep:tl`), flag
  name `html-fallback` unchanged. Tests: `state_script_keeps_entities_and_escapes_verbatim`
  added, existing 48 lib tests unchanged.

## Alternatives considered

- **Option A: keep `scraper` and actually use it** — rejected: strictly worse
  than `tl` (no SIMD, bigger transitive graph).
- **Option B: keep the hand-rolled scanner, drop the `scraper` dep** —
  rejected: it would keep the two bugs above for a dependency-free win we can
  already get with `tl`.
- **Option C: drop the `v2_html` extractor entirely** — rejected: it is the
  fallback path when the JSON endpoint is unavailable, which is the exact
  scenario this project exists for.
