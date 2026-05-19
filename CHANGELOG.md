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

- Added: `CHANGELOG.md` release history and maintainer guidance.
- Added: Cross-platform release workflow and documented prebuilt-binary install
  paths for macOS, Linux, and Windows.
- Added: `itr ui`, a local browser editor served from the Rust binary with a
  localhost JSON API.
- Docs: Expanded install, UI, and agent workflow documentation.

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
- Docs: Documented search semantics and soft-fallback philosophy.

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

### Release notes

- Added: Bootstrap version tag retained for git-based release history.

### Upgrade notes

- Prefer the latest `v*` tag for installs.
