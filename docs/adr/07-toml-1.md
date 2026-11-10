# 07. Migrate config parsing to toml 1.2

- **Status:** Accepted
- **Date:** 2026-11-10
- **Deciders:** @Githab-capibara
- **Related:** [config-reference.md](../config-reference.md)

## Context

`toml = "0.8"` is superseded by the merged `toml` 1.x line (TOML 1.1 + spec
versioned releases). 0.8 receives no new fixes, and the broader migration plan
wreq/sonic-rs/tl already pulls modern transitive versions; keeping an orphan
major blocks nothing but adds one more version skew to audit.

Only one call site exists: `load_config` in `src/main.rs` (`toml::from_str`).

## Decision

Bump to `toml = "1.2"` and use `toml::de::Error::span()` to render parse
failures with line/column, replacing the flat `with_context` message.

## Consequences

- **Easier:** parse errors now name the exact position
  (`parse config x.toml: invalid type: string "текст", expected u32 (line 3, column 15)`),
  which is what users hit when hand-editing `config.toml`.
- **Easier:** one fewer stale major in the tree.
- **Harder:** banner errors changed shape — `validate-config` output assertions
  in tooling must match the new text.
- **Given up:** nothing; `from_str` signature unchanged, 0.8 → 1.x is a Cargo
  bump for this codebase.
- **Migration:** done, no config rewrite needed (TOML 1.0 superset of what we
  parse).

## Alternatives considered

- **Option A: stay on 0.8** — rejected: no upstream maintenance, and every
  other crate we plan to adopt is on current versions.
- **Option B: switch to `toml_edit` directly** — rejected for now: we only read
  config, never write it back, so format-preserving editing buys nothing yet.
  Revisit if a `config set` command appears.
