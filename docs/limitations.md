# Known Limitations And Roadmap

This document separates intentional constraints from bugs and future work. It is
not a release plan; it is a guide for contributors deciding whether a behavior
is expected.

## Intentional Constraints

These are current design choices.

### Local-Only Operation

`itr` stores state in a local `.itr.db` file. There is no required network,
sync service, cloud backend, or hosted issue tracker.

Export/import exists for portability. Distributed sync is not part of the core
runtime.

### No Daemon

Each CLI command opens the database, performs work, prints output, and exits.
`itr ui` is the exception: it runs a foreground localhost server until stopped.

### No Auth System

The core CLI has no users, accounts, or permissions. `ITR_AGENT` is attribution,
not authentication.

The UI uses a per-session token for localhost API requests, but it is not a
remote multi-user security model. See [security.md](security.md).

### Single Binary

The project should remain a single Rust binary with bundled SQLite. Avoid
required external runtimes, services, or package managers.

### Dependency-Light UI

The UI is embedded vanilla HTML, CSS, and JavaScript. It intentionally has no
Node, frontend build step, Electron, Tauri, async runtime, or webview framework.

### No Hard Delete In UI

The UI does not expose hard issue deletion. Cleanup workflows should resolve,
wontfix, or tag issues for review. Notes can be edited and deleted.

### Parseable Output First

stdout is data, stderr is diagnostics. Human-friendly output is useful, but it
must not break scripts or agent workflows.

### Soft Fallback

For recoverable bad input, `itr` often normalizes, defaults, and marks data for
review instead of hard failing. This is expected behavior, not a bug.

## Compatibility Regression Tests

The end of `tests/integration.sh` contains tests that started as known-bug
reproducers. Keep them as compatibility coverage even after the behavior is
fixed. If one fails again, treat it as a regression against the documented
contract.

### `itr deps` Alias

Contract: `itr deps <ID> --on <ID>` works as an alias for `itr depend`.

Regression coverage: issue #42.

### Repeated `-t` Tags

Contract: `itr add "title" -t bug -t test` accepts repeated `-t` values for
multiple tags.

Regression coverage: issues #43/#46. Repeatable tag input should keep working
without breaking existing `--tags` comma-separated input.

### Dash-Prefixed Acceptance Values

Contract: `itr add "title" --acceptance "-t flag works correctly"` accepts the
dash-prefixed acceptance string as data.

Regression coverage: issue #45.

If a new unresolved bug is documented in this area, make the status explicit and
link the `itr` issue that tracks it.

## Roadmap Themes

These are likely future directions, not committed scope.

### Documentation Completeness

Contributor and user docs for architecture, schema, command contracts, UI API,
testing, security, backup/import/export, troubleshooting, changelog, and
roadmap have shipped and live under `docs/`. Ongoing work is maintenance: keep
these in sync with command behavior, the embedded skill, and the agent guide
when shipping changes.

### Data Portability

Export/import is the current portability path. Future work could improve
round-trip coverage, add stronger verification, or document deliberate gaps.

### Optional Sync

`itr-plan.md` discusses optional JSONL/git-style sync as future work. If added,
it should remain opt-in. The SQLite database should stay the local source of
truth.

### Issue Templates

`itr-plan.md` sketches template support for recurring issue types. This would
need to preserve the current no-config/zero-setup feel or remain optional.

### External Tracker Bridges

Bridges to GitHub Issues or other systems are possible future work. They should
not make the local CLI depend on network access.

### More Focused Tests

The integration suite carries most coverage. Pure helpers can use Rust unit
tests where that gives faster and clearer feedback.

## Reading `itr-plan.md`

`itr-plan.md` is planning context, not the current contract. Some details have
changed since implementation. Treat implemented code, integration tests,
`README.md`, `CONTRIBUTING.md`, and docs under `docs/` as authoritative for
current behavior.

Use `itr-plan.md` mainly for:

- original product intent;
- possible future features;
- rationale for local-first, agent-first design.

Do not copy old command syntax from `itr-plan.md` without checking `itr --help`
or `src/cli.rs`.
