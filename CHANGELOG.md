# Changelog

All notable user-facing changes are recorded here.

## Versioning

- Release tags use `vMAJOR.MINOR.PATCH`.
- Pushes to `main` are auto-tagged by `.github/workflows/auto-version.yml`.
- A conventional commit subject with `!` after the type or scope creates a major
  bump. A `feat:` subject creates a minor bump. A `fix:` subject creates a patch
  bump. Commits without those subjects do not create a tag.
- Add `[skip version]` to the commit message to skip auto-tagging.
- `.github/workflows/release.yml` builds release archives and SHA256 files from
  existing `v*` tags. GitHub release notes are generated automatically; this file
  is the terse maintained history.
- Built binaries embed `git describe --tags --always --dirty` through `build.rs`,
  falling back to the Cargo package version when git metadata is unavailable.
- `Cargo.toml` intentionally pins `package.version` at `0.1.0` and does not track
  the latest `v*` tag. Git tags are the source of truth for releases; `build.rs`
  surfaces the tag-derived version at runtime. The Cargo field is only used as a
  fallback for source builds with no git metadata, so bumping it on every release
  would add churn without changing user-visible behavior. See the `0.1.0` Cargo
  pin and the `v0.1.0` git tag (re-dated `2026-03-07` to mark the bootstrap point
  for git-based history — see that entry below).

## Entry Format

- Keep newest sections first.
- Use `### Release notes` for user-visible behavior, commands, docs, release
  artifacts, and fixes.
- Use `### Upgrade notes` for compatibility, install, migration, and operator
  actions.
- Group release-note bullets as `Added`, `Changed`, `Fixed`, `Docs`, or `CI`
  when a release has more than one kind of change.

## Unreleased

### Release notes

- Added: multi-ID mutating verbs — `close`, `note`, `relate`, and `depend` now
  accept repeated IDs, comma lists, and inclusive `A-B` ranges (e.g.
  `itr close 12,14,17 "fixed"`, `itr relate 124-132 --to 53`) in one
  transaction with per-ID soft fallback; `get`/`show` gain the same range
  syntax. `claim` deliberately stays single-ID.
- Added: filter-based `bulk relate`, `bulk depend`, and `bulk note` with the
  shared bulk filter grammar and `--dry-run` on all three (dry runs execute
  the real code path in a rolled-back transaction, so validation cannot
  drift and nothing is written).
- Added: `batch add --dry-run` and `batch note --dry-run` — per-item verdicts
  (including resolved priority/kind defaults and `@N` references) matching
  the real run, with no writes.
- Added: `--fields` now works on all four formats for issue lists and honors
  the requested field order — `oneline` emits chosen columns as
  tab-separated script-ready output, `pretty` builds its table columns from
  the list (with extra columns like `tags`/`created_at` available), and JSON
  re-serializes surviving keys in the requested order.
- Added: dependency edges and note additions now record audit events
  (`dependency_added`, `dependency_removed`, `note_added`), so every
  multi-ID/bulk mutation shows up in `itr log`.
- Changed: JSON `Value`-built output (ad-hoc `json!` objects, `--fields`
  filtered output, `close`/`update` detail envelopes) now serializes in
  insertion/struct order instead of alphabetical key order (serde_json
  `preserve_order`); `stats -f json` keeps its documented alphabetical
  byte-stable contract.
- Docs: README, `itr agent-info`, and `docs/command-contracts.md` updated for
  multi-ID syntax, the new bulk verbs, batch dry-runs, and fields-everywhere;
  fixed the stale claim that `batch add` is all-or-nothing (per-item soft
  fallback since #164) and added a "which one do I want" guide for
  multi-ID vs `bulk` vs `batch`.
- Added: the embedded agent skill (`itr skill`) now covers filing issues from
  failed gates — pull evidence from `gatr last` / `gatr errors` instead of
  re-running builds or pasting raw logs.
- Added: `CHANGELOG.md` release history and maintainer guidance.
- Added: Cross-platform release workflow and documented prebuilt-binary install
  paths for macOS, Linux, and Windows.
- Added: `itr ui`, a local browser editor served from the Rust binary with a
  localhost JSON API.
- Docs: Expanded install, UI, and agent workflow documentation.
- Docs: Added a GitHub Actions CI badge to the top of `README.md`.

### Upgrade notes

- No database migration or CLI action is required for the documented changes.
- After the next tagged release, installers can fetch prebuilt archives. Source
  installs continue to work.

## v2.9.6 - 2026-03-12

### Release notes

- Fixed: CI uses `rustup` directly instead of `dtolnay/rust-toolchain`.

### Upgrade notes

- No user action required.

## v2.9.5 - 2026-03-12

### Release notes

- Fixed: Rust toolchain pinning eliminates CI format mismatch.

### Upgrade notes

- No user action required.

## v2.9.4 - 2026-03-12

### Release notes

- Fixed: Removed deprecated `deny.toml` keys.
- CI: Added `cargo-deny` to local CI coverage.

### Upgrade notes

- No user action required.

## v2.9.3 - 2026-03-12

### Release notes

- Fixed: Updated `deny.toml` for cargo-deny v2.

### Upgrade notes

- No user action required.

## v2.9.2 - 2026-03-12

### Release notes

- Fixed: CI format checks.
- CI: Updated GitHub Actions usage for Node.js 24.

### Upgrade notes

- No user action required.

## v2.9.1 - 2026-03-12

### Release notes

- Fixed: `itr upgrade` now prints progress while rebuilding.

### Upgrade notes

- No user action required.

## v2.9.0 - 2026-03-12

### Release notes

- Added: `itr close` accepts `--reason` as a soft-fallback alias.
- Changed: List filtering internals were simplified without expected output
  changes.

### Upgrade notes

- Existing close command forms continue to work.

## v2.8.2 - 2026-03-12

### Release notes

- Fixed: Clippy pedantic warnings are clean across the codebase.

### Upgrade notes

- No user action required.

## v2.8.1 - 2026-03-12

### Release notes

- Fixed: Rust best-practices audit cleanup.

### Upgrade notes

- No user action required.

## v2.8.0 - 2026-03-11

### Release notes

- Added: `--title`, `--body`, and `batch create` soft-fallback aliases.

### Upgrade notes

- Existing `add` and `batch add` command forms continue to work.

## v2.7.0 - 2026-03-08

### Release notes

- Added: `itr summary` for project narrative output.
- Added: `itr wip` for in-progress issue lists.
- Added: `note-delete` and `note-update`.
- Added: `--tag-any` OR filtering.
- Added: `--agent` filtering for `itr log`.
- Added: `oneline` output for greppable issue lists.
- Fixed: Dependency edges are cleaned when issues close.
- Fixed: Claim audit events are recorded.
- Fixed: `--fields` validation and list field output.
- Fixed: `assigned_to` appears in pretty list output.
- Docs: Documented search semantics and soft-fallback philosophy. See
  [`docs/search.md`](docs/search.md) for FTS5 vs LIKE dispatch, indexed fields,
  AND-by-default semantics, and `itr reindex` guidance.

### Upgrade notes

- No manual migration is required.
- Scripts that parse list output can use `--fields` and `oneline` for narrower,
  stable output.

## v2.6.2 - 2026-03-07

### Release notes

- Fixed: CLI parsing bugs from issues #42, #43/#46, and #45.
- Changed: Shared command logic was extracted for safer behavior.

### Upgrade notes

- No user action required.

## v2.6.1 - 2026-03-07

### Release notes

- Fixed: `--fields` restricts output for all formats, not only JSON.

### Upgrade notes

- Scripts using `--fields` should expect narrower output consistently across
  formats.

## v2.6.0 - 2026-03-07

### Release notes

- Added: Repeatable `--tag`, `--file`, and `--skill` aliases on add and update.
- Fixed: Removed implicit stdin reads that could hang command chains.
- Changed: Shared utility functions and unit tests were extracted.

### Upgrade notes

- Pipelines should pass stdin only to commands that explicitly document stdin
  input.

## v2.5.0 - 2026-03-07

### Release notes

- Added: `claim` and `start` accept an optional issue ID argument.

### Upgrade notes

- Existing `claim` and `start` usage continues to work.

## v2.4.0 - 2026-03-07

### Release notes

- Added: `batch note` for bulk note creation.
- Added: `--dry-run` for `batch close` and `batch update`.
- Added: Close reason output in `batch close` compact output.
- Changed: `batch add` output uses a `BatchResult` envelope.

### Upgrade notes

- Scripts parsing `batch add` output should read the `BatchResult` envelope.

## v2.3.0 - 2026-03-07

### Release notes

- Added: `--fields` support for batch result output.
- Added: End-to-end tests.

### Upgrade notes

- No user action required.

## v2.2.0 - 2026-03-07

### Release notes

- Added: Git-based versioning and auto-tagging.
- Fixed: Auto-version workflow fetches tags before calculating the next version.

### Upgrade notes

- Builds from a git checkout report `git describe` version metadata.

## v2.1.1 - 2026-03-07

### Release notes

- Fixed: Batch command behavior.

### Upgrade notes

- No user action required.

## v2.1.0 - 2026-03-03

### Release notes

- Added: Expanded self-documentation.

### Upgrade notes

- No user action required.

## v2.0.0 - 2026-03-02

### Release notes

- Added: Multi-agent support.

### Upgrade notes

- Review agent attribution workflows before sharing one project database across
  multiple workers.

## v1.2.0 - 2026-03-02

### Release notes

- Added: Claude Code skill support.

### Upgrade notes

- Run `itr skill install` when agents should auto-discover `itr` guidance.

## v1.1.0 - 2026-03-01

### Release notes

- Added: LIKE-backed search.

### Upgrade notes

- No user action required.

## v1.0.0 - 2026-03-01

### Release notes

- Added: Initial stable CLI release.
- Added: Soft-fallback behavior.
- Fixed: `itr upgrade`.
- Fixed: Dash slicing in display output.

### Upgrade notes

- Invoke the binary as `itr` from `PATH`.

## v0.1.0 - 2026-03-07

> Ordering note: this entry is intentionally placed at the bottom in semver
> order, not in date order. The `v0.1.0` tag was cut on `2026-03-07` (after
> `v1.0.0` on `2026-03-01`) as a retroactive bootstrap marker for the
> auto-version workflow's git-describe baseline; treating it as the lowest
> version keeps the changelog scannable by release number. All later entries
> follow strict newest-first ordering.

### Release notes

- Added: Bootstrap version tag retained for git-based release history.

### Upgrade notes

- Prefer the latest `v*` tag for installs.
- The `Cargo.toml` `package.version` field is intentionally pinned at `0.1.0`;
  release versions come from git tags via `build.rs`. See the Versioning
  section at the top of this file.
