# Blitz log — 2026-05-18 doc verification

## Config

- Mode: verification (read-only)
- Tracker: n/a (no issue closures)
- Verify gate: n/a (no code mutations)
- Concurrency: 5
- Repos: `.`
- Scope: all 19 project docs + 1 cross-doc completeness auditor = 20 agents
- Bias control: agents work blind — no prior single-agent audit findings shared

## Stop conditions

- All 20 agents reach terminal state, OR
- Two consecutive waves produce zero structured reports

## Waves

### Wave 1 — technical source-of-truth docs (5 agents)

| Doc | Source-of-truth cross-check |
|---|---|
| `docs/schema.md` | `src/db.rs`, `src/models.rs` |
| `docs/ui-api.md` | `src/commands/ui.rs` |
| `docs/command-contracts.md` | `src/cli.rs`, `src/main.rs` |
| `docs/architecture.md` | `src/*` module tree |
| `docs/security.md` | `src/commands/ui.rs` token logic |

### Wave 2 — operational docs (5 agents)

| Doc | Source-of-truth cross-check |
|---|---|
| `docs/backup-import-export.md` | `src/commands/export.rs`, `src/commands/import.rs` |
| `docs/testing.md` | `tests/integration.sh`, `justfile` |
| `docs/troubleshooting.md` | error paths, install scripts |
| `CHANGELOG.md` | `git log`, release workflows |
| `docs/roadmap.md` | `itr-plan.md`, known-bug tests |

### Wave 3 — entry-point & agent docs (5 agents)

| Doc | Source-of-truth cross-check |
|---|---|
| `README.md` | `src/cli.rs`, `src/agent_docs.rs` (all commands referenced?) |
| `AGENTS.md` | `install.sh`, project conventions |
| `CONTRIBUTING.md` | `justfile`, build/test workflow |
| `skills/itr/SKILL.md` | `itr add` behavior |
| `CLAUDE.md` | repo module state, current commands |

### Wave 4 — meta / style / pre-existing (5 agents)

| Doc | Source-of-truth cross-check |
|---|---|
| `STORY_STYLE.md` | sampling of recent `itr` issues for conformance |
| `RUST_SCORE.md` | current `src/*` Rust practices (audit may be stale) |
| `itr-plan.md` | current architecture vs. original design |
| `docs/soft_fallbacks.md` | `src/commands/add.rs`/`update.rs` soft-fallback impl |
| **Completeness auditor** | reads all 19 docs, identifies set-level gaps |

## File conflicts

None — read-only verification, no file edits.

## Semantic warnings

- Wave 1 agents will all hit the same source files (`src/db.rs`, `src/commands/ui.rs`, `src/cli.rs`). No conflict — concurrent reads are safe.
- Pre-existing docs (Wave 4) are NOT scoped to the Codex doc-pass; agents should flag if these are stale vs. current state rather than assuming they're authoritative.

## Interventions

(empty)

## Outcomes

### Wave 1 (in flight)

- ✅ `docs/schema.md` — **ACCURATE**. All 7 tables, migration order, pragmas, JSON-in-TEXT, FTS5 fallback, note/dependency audit-event gap all verified against `src/db.rs`. Zero follow-ups. Minor nit: `PRAGMA` attribution wording (in SCHEMA const + re-executed in `open_db`) is technically correct.
- ✅ `docs/ui-api.md` — **MINOR ISSUES**. All 20 routes documented (3 static + 17 API — verified inventory differs from initial estimate by +1 due to `POST /api/sql` dangerous-mode route). 6 follow-ups suggested:
  - Document alphabetic priority sort surprise (medium)
  - Document PATCH `status` does not unblock dependents (medium) — easy consumer footgun
  - Add `403 DANGEROUS_SQL_DISABLED` to error table (low)
  - Document `X-Content-Type-Options` / `Referrer-Policy` headers (low)
  - Describe token generation (24 random bytes hex) + process-bound lifetime (low)
  - Clarify empty-string `assigned_to` filter behavior (low)
- ✅ `docs/command-contracts.md` — **ACCURATE**. All 37 `Commands` enum variants + 7 visible aliases + all 4 subcommand enums covered. JSON shapes spot-verified. Soft-fallback documentation matches `normalize.rs` and `src/main.rs:125-136,257-268`. DB-path precedence (`ITR_DB_PATH` > `--db` > walk-up) and init's inverted precedence (`--db` > env > cwd) both correctly documented. 2 follow-ups:
  - **SOURCE BUG**: `src/main.rs:42-47` error message says "Valid: compact, json, pretty" but `oneline` is also accepted (low) — code issue, not doc issue
  - Add "See also" cross-links from command-contracts.md to soft_fallbacks.md / architecture.md (low)

### Wave 1 summary
- Docs verified: 5/5 ✅
- Verdicts: 2 ACCURATE, 3 MINOR ISSUES, 0 INACCURATE, 0 INCOMPLETE
- Total follow-ups queued: 15 (1 source-code bug, 14 doc refinements — most low-severity)
- No quarantines, no interventions, no verify-gate (read-only)
- Wave gate: ✅ green (no verify gate applicable)

### Wave 2 (in flight)

- ✅ `docs/backup-import-export.md` — **ACCURATE**. Default JSONL format, JSON pretty-print, stdin fallback, `INSERT OR REPLACE` for issues/notes, `INSERT OR IGNORE` for dependencies, `--merge` skip-existing semantics all verified. Round-trip integrity correctly documented: export emits events + relations but import silently drops them. 2 follow-ups:
  - **CLI silently drops events/relations on import** (medium) — doc warns but CLI doesn't; either implement import for those tables or emit a `REVIEW:` stderr warning. Real footgun for users restoring audit history.
  - Mention WAL companion files (`.itr.db-wal`, `.itr.db-shm`) in "What To Back Up" (low-medium) — `cp` while `itr ui` is running can yield inconsistent snapshot
- ✅ `docs/testing.md` — **ACCURATE**. just recipes, integration.sh release-default, python3 (not jq) JSON parsing, unit test references (`util::tests::parse_comma_list_basic`, `format::tests::pretty_list_with_em_dash_title_does_not_panic`) all verified to exist. CI recipe order matches. 3 follow-ups (all low):
  - UI coverage list omits `/api/bootstrap` and `/api/sql` (--allow-dangerous) paths
  - `just verify` (full pre-push gate) not mentioned
  - No cross-doc links — could point to `ui-api.md`, `troubleshooting.md`
  - **CROSS-DOC FINDING**: CLAUDE.md is stale — says "no unit tests yet" but unit tests exist at `src/util.rs:66` and `src/format.rs:975`
- ⏳ `docs/troubleshooting.md` — running
- ✅ `CHANGELOG.md` — **MINOR ISSUES**. All 25 git tags have entries; every entry has a tag (no orphans). Unreleased section accurately covers the 4 commits since v2.9.6. 3 follow-ups:
  - **Cargo.toml version drift** (low-medium): Cargo.toml says `0.1.0` but latest tag is `v2.9.6`. CHANGELOG mentions `git describe` is the source of truth but doesn't document this intentional drift. Could be a foot-gun in offline/no-git builds.
  - v0.1.0 ordering ambiguity (low): listed at bottom (semver order) but dated 2026-03-07, AFTER v1.0.0 (2026-03-01). Document the bootstrap-tag rationale or reorder.
  - Missing README CI-badge entry (db7e324) in Unreleased > Docs (trivial)
- ✅ `docs/troubleshooting.md` — **MINOR ISSUES**. `ITR_VERSION` / `ITR_INSTALL_DIR` / `ITR_FROM_SOURCE` env vars, `itr upgrade --source-dir --no-pull`, `itr doctor --fix` semantics (orphan deps + stale blockers + FTS rebuild) all verified against source. 4 follow-ups:
  - **install.sh `--update` flag undocumented** (medium) — the recent `--update` workflow + curl-update pattern is missing from the install/PATH problem guide
  - **WAL side files not mentioned** (medium) — `.itr.db-wal`/`.itr.db-shm` get no troubleshooting coverage; cross-cuts with backup-import-export's same gap
  - **`choose_install_dir` precedence wrong** (medium) — doc claims `~/.local/bin` or `~/.cargo/bin`, but install.sh actually picks the existing-itr-on-PATH directory first
  - Error-code reference table missing (low) — only `INVALID_VALUE` is named; missing `NO_DATABASE`, `CYCLE_DETECTED`, `NOT_FOUND`, `UPGRADE_FAILED`, `NO_FILTERS`, `DB_ERROR`, `PARSE_ERROR`, `IO_ERROR`
- ✅ `docs/roadmap.md` — **MINOR ISSUES**. All 8 v1 constraints verified against source. Regression-coverage issue refs (#42/#43/#45/#46) verified in `tests/integration.sh:1291-1333`. itr-plan.md citations all accurate. 2 follow-ups:
  - **Filename collision with `/roadmap` skill artifact (medium)** — independently re-confirmed by reading both cases and getting identical content. README.md:138 links to `docs/roadmap.md` so any rename must update that link. Recommendation: rename to `docs/known-limitations.md`.
  - "Documentation Completeness" roadmap theme reads as pending but shipped in 671bed9 (low)

### Wave 2 summary
- Docs verified: 5/5 ✅
- Verdicts: 2 ACCURATE, 3 MINOR ISSUES, 0 INACCURATE, 0 INCOMPLETE
- Follow-ups: 14 (running total: 29)
- Wave gate: ✅ green (no verify gate applicable)

### Cross-cutting findings (Wave 1+2)
- **WAL side files undocumented** (`backup-import-export.md`, `troubleshooting.md`) — same gap surfaces twice
- **install.sh `--update` flag undocumented** (`troubleshooting.md` confirmed; AGENTS.md partial coverage pending Wave 3 check)
- **Filename collision** confirmed by 2 independent agents
- **CLAUDE.md drift** — agent verifying testing.md surfaced that CLAUDE.md still says "no unit tests yet" but `src/util.rs:66` and `src/format.rs:975` have unit tests
- **Cargo.toml version drift** — Cargo.toml `0.1.0` vs latest tag `v2.9.6`; CHANGELOG hedges with "git describe" but doesn't formally document the divergence

### Wave 3 (in flight)

- ✅ `README.md` — **INCOMPLETE** (first non-MINOR verdict). 14 commands missing (assign, unassign, log, relate, unrelate, reindex, search, batch close/update/note, bulk close/update, note-delete, note-update, summary, wip/current). `ITR_AGENT` and `--fields` missing. 8 follow-ups:
  - **NEW BUG SURFACED**: README:350 says `notes_count` urgency coefficient maxes at 3.0; actual cap is **0.5** per `urgency.rs:188-189` (`(notes / 6.0).min(1.0) * 0.5`) — **6x error in user-tunable documentation** (medium)
  - 14 commands missing (high) — matches #93
  - `ITR_AGENT` env var absent (high)
  - `--fields` global flag absent from Global Flags section (medium)
  - README:384 "Four tables" undersells schema; `events`, `relations`, migration-added columns omitted (medium)
  - `oneline` format option missing from README:266 (low)
  - `add` flag list omits `skills` and `assigned_to` (medium)
  - Documentation index missing links to `CONTRIBUTING.md`, `AGENTS.md`, `docs/soft_fallbacks.md` (low)
- ✅ `AGENTS.md` — **MINOR ISSUES**. Build/test commands, UI rules (no Node/Tauri/async), hard-delete claim, `ITR_AGENT` plumbing all verified. 3 follow-ups:
  - **install.ps1 lacks --update flag / PATH-aware logic** (medium) — confirms cross-cutting gap. install.sh has both; install.ps1 has neither. AGENTS.md's "check install.sh and install.ps1" guidance misleads on Windows.
  - `itr skill` / `itr skill install` not mentioned in AGENTS.md (low)
  - `cargo deny` not in "Common checks" block despite being in `verify`/`ci` justfile recipes (low)
- ✅ `CONTRIBUTING.md` — **ACCURATE**. All 9 invariants verified against source. `justfile` recipe `verify: release lint test fmt-check deny` matches line-for-line. UI auth contract (`X-ITR-Token`, `require_token` at `ui.rs:499-510`) accurate. All 14 cross-doc links resolve. 2 follow-ups (both low):
  - Doesn't link to `STORY_STYLE.md` (may be intentional — style guide is for sprint planning, not contributor onboarding)
  - No dedicated "Commit Messages" subsection despite `auto-version.yml` parsing `feat:`/`fix:`/`type!:` prefixes — low; functional but discoverable
  - **CROSS-DOC CONFIRMATION**: Second independent agent verifies unit tests at `util.rs:59-189` (many `#[test]` cases), CLAUDE.md's "no unit tests yet" claim is definitively stale
- ✅ `skills/itr/SKILL.md` — **ACCURATE**. `include_str!("../../skills/itr/SKILL.md")` resolves correctly. All `itr add` flags listed match `cli.rs:37-100`. Priority values match `normalize.rs:37`; kind values match `normalize.rs:48`. `BatchAddInput` schema match verified. STORY_STYLE.md exists. ZERO follow-ups.
  - Optional polish: doc could mention `--assigned-to` since `ITR_AGENT` only attributes audit log, not assignee

### Wave 3 summary
- Docs verified: 5/5 ✅
- Verdicts: 2 ACCURATE, 2 MINOR ISSUES, 1 INCOMPLETE (README.md)
- Follow-ups: 19 (running total: 48)
- Notable: SKILL.md cleanest; README most degraded
- Wave gate: ✅ green (no verify gate applicable)

### Wave 4 (in flight)

- ✅ `docs/soft_fallbacks.md` — **MINOR ISSUES** (essay; no project-specific code claims). REVIEW: convention verified across 18 occurrences in src/. add.rs/update.rs/batch.rs reference patterns all present. 3 low-severity follow-ups (editorial):
  - **Self-undermining claim**: doc says "no framework treats this as a first-class pattern" but `itr` itself does — REVIEW notes + `_needs_review` tag + batch outcomes (ok/review/error). Cite `itr` as a counter-example or qualify.
  - Add project-specific epilogue showing `itr`'s soft-fallback implementation
  - Add cross-doc links to/from CLAUDE.md "Soft Fallbacks Philosophy" — essay is currently unlinked from rest of doc tree
- ✅ `STORY_STYLE.md` — **ACCURATE**. Title imperative form 10/10, 80-char cap holds, "Context:" pattern verified in recent issues (#83-#93 all conform), bulleted observable acceptance criteria verified, tag taxonomy verified flat with documented examples present (`ci`, `release`, `testing`, `docs`, etc.), priority values exact match. Banned phrases ("works properly", "handles things better", "simply") all returned zero hits. ZERO follow-ups.
  - Optional polish: legacy `Bug:` / `Refactor:` / `Cleanup:` prefix convention (older issues #39-#57) is no longer used; doc could note the deprecation
- ✅ `RUST_SCORE.md` — **STALE-BUT-USEFUL**. Doc pinned to 2026-03-12 / commit `61799ed`. **6 of 9 action items have shipped** since: release profile (`Cargo.toml:8-11`), cargo-deny (deny.toml + CI), urgency.rs REVIEW notes, clippy-pedantic config, `with_capacity` rollout (16 sites), `ListFilter` struct extraction. 3 higher-effort items unfinished (proptest, doctests, clone reduction in `build_issue_summary`). 3 follow-ups:
  - Refresh against current `main` (medium) — sections 2/3/4/6 obsolete, LOC ~7,500 → 8,901 actual, `db.rs` 1072 not 730
  - `the_comprehensive_rust_best_practices_reference.md` referenced but doesn't exist in repo (low) — dangling link
  - Decide on remaining higher-effort items (low) — open question for next audit
- ✅ `itr-plan.md` — **DESIGN-DOC-EXPECTED-DRIFT**. Most v0.1 commands shipped + ~30 commands beyond planned. 5 follow-ups, most medium:
  - **No disclaimer / "as of" framing** (medium) — reads as authoritative spec; new contributors will treat it as ground truth and get the wrong exit-code contract, no-UI claim, and `STALE_IN_PROGRESS` code
  - **Exit code 2 in plan = empty results, but reality is empty = exit 0** (medium) — directly contradicts shipped `error.rs::print_empty`
  - MCP future-work uses `nit_*` tool prefix (old project name) (low)
  - Plan example `itr export --format json` doesn't toggle JSONL/JSON under current CLI (low) — `--export-format` is the actual flag
  - Schema additions (skills, assigned_to, events, relations, FTS) absent from plan's schema section (low/medium)
- ✅ Cross-doc completeness auditor — **PARTIALLY COMPLETE**. Highest-value findings (catches what per-doc agents can't):
  - **4 genuinely missing docs** (medium each):
    - `docs/urgency.md` — formula only in README; agents reading SKILL → `itr agent-info` lack authoritative coverage
    - `docs/environment.md` (or README section) — `ITR_*` vars scattered across 4 docs
    - `docs/migrations.md` — developer how-to walkthrough; rules exist but no example
    - `docs/search.md` — FTS5 vs LIKE semantics, AND-by-default, what's indexed
  - **5 orphan docs** (no inbound README link): `STORY_STYLE.md`, `RUST_SCORE.md`, `docs/soft_fallbacks.md`, `itr-plan.md`, `itr agent-info`
  - **Cross-cutting drift**: README implies `ITR_DB_PATH` ≡ `--db`, but command-contracts.md says env wins (with `init` inverted)
  - **CLAUDE.md ↔ docs/architecture.md ↔ CONTRIBUTING.md 3-way architecture overlap** — CLAUDE.md should reference, not restate
  - README's "task" used loosely as synonym for "issue" violates STORY_STYLE.md's "prefer 'issue'" rule
  - `itr-plan.md` not internally marked as historical (only `docs/roadmap.md` notes this)
  - **Terminology drift**: `blocked-by` (README field list) vs `blocked_by` (JSON examples + ui-api.md) — both technically correct, but readers will stumble

### Wave 4 summary
- Docs verified: 5/5 ✅
- Verdicts: 1 ACCURATE, 1 MINOR ISSUES, 1 STALE-BUT-USEFUL, 1 DESIGN-DOC-EXPECTED-DRIFT, 1 PARTIALLY COMPLETE
- Follow-ups: 19 (Wave 4 total)
- Wave gate: ✅ green (no verify gate applicable)

---

## Final synthesis

### Total verdicts across all 19 docs + 1 completeness audit

| Verdict | Count | Docs |
|---|---|---|
| ACCURATE (zero substantive follow-ups) | 3 | `schema.md`, `SKILL.md`, `STORY_STYLE.md` |
| ACCURATE (with low-severity follow-ups) | 4 | `command-contracts.md`, `backup-import-export.md`, `testing.md`, `CONTRIBUTING.md` |
| MINOR ISSUES | 9 | `ui-api.md`, `security.md`, `architecture.md`, `troubleshooting.md`, `CHANGELOG.md`, `roadmap.md`, `AGENTS.md`, `CLAUDE.md`, `soft_fallbacks.md` |
| STALE-BUT-USEFUL | 1 | `RUST_SCORE.md` (snapshot pinned to commit `61799ed`; 6 of 9 action items shipped) |
| DESIGN-DOC-EXPECTED-DRIFT | 1 | `itr-plan.md` (most v0.1 commands shipped + ~30 commands beyond) |
| INCOMPLETE | 1 | `README.md` (14 commands missing, env var missing, real urgency-bug) |

### Source-code bugs surfaced by docs verification (not just doc bugs)

1. **`src/main.rs:42-47`** — invalid-format error message says "Valid: compact, json, pretty" but `oneline` is also accepted (per `src/format.rs:65-73`). Code bug, surfaced by `command-contracts.md` verification.
2. **`README.md:350` urgency `notes_count` "max 3.0"** — actual cap is **0.5** per `urgency.rs:188-189` (`(notes / 6.0).min(1.0) * 0.5`). 6x error in user-tunable documentation. Could mislead config tuning.
3. **Importer silently drops events/relations** — export emits them, import never restores them (`src/commands/import.rs`). Doc warns; CLI is silent. Suggests REVIEW: stderr warning.
4. **install.ps1 lacks `--update` flag + PATH-aware detection** that install.sh now has. Platform asymmetry — Windows users have no update workflow.

### Cross-cutting findings (multi-agent corroboration)

- **CLAUDE.md "no unit tests yet" is stale** — confirmed by 3 independent agents (`testing.md`, `CONTRIBUTING.md`, `CLAUDE.md` agents). Tests at `src/util.rs:59` (23) and `src/format.rs:878` (12) — 35 unit tests total.
- **Filename collision: `docs/roadmap.md` blocks `/roadmap` skill's `docs/ROADMAP.md`** on APFS — confirmed by 2 independent agents.
- **WAL side files (`.itr.db-wal`, `.itr.db-shm`) undocumented** — appears in both `backup-import-export.md` and `troubleshooting.md` gaps.
- **`install.sh --update` flag undocumented** — gap in `troubleshooting.md`; partial in `AGENTS.md`.

### 4 net-new docs proposed

- `docs/urgency.md` — coefficient table + math + worked example
- `docs/environment.md` (or merge into README) — `ITR_AGENT`, `ITR_DB_PATH`, `ITR_VERSION`, `ITR_INSTALL_DIR`, `ITR_FROM_SOURCE`, `ITR_SOURCE_DIR`
- `docs/migrations.md` — developer how-to
- `docs/search.md` — FTS5 / LIKE semantics

### Interventions log

(empty — no quarantines, no failed verify gates, no permission failures)

### Diff summary

No source files modified. Only `sprint/_unscoped/blitz-2026-05-18-verify.md` (this log) was written. All 20 agents were strictly read-only.

### Next steps

- File follow-ups in `itr` (~30-40 unique tickets after dedupe)
- Decide on uncommitted Codex changes (still untracked: 9 new docs + 4 modified files + install.sh feature)
- Decide what to do about `itr-plan.md` and `RUST_SCORE.md` (snapshot/historical docs — disclaimers or refresh)
- ✅ `CLAUDE.md` — **MINOR ISSUES**. 6 follow-ups, several from edits made earlier in this session:
  - **"no unit tests yet" claim stale** (medium) — independently confirmed for the 3rd time; `src/util.rs:59` has 23 `#[test]` fns + `src/format.rs:878` has 12 — 35 total
  - **`util.rs` missing from Key Modules** (medium) — declared at `main.rs:10` with tag/skill/date helpers
  - **DB-not-required command list is wrong**: doc says 3 (init/schema/upgrade); actual at `main.rs:62-84` is 5 — `agent-info` and `skill` also exempt (medium). Self-induced: I added `commands/skill.rs` to the module list but didn't update the dispatch sentence.
  - **`skills`/`assigned_to` listed alongside `events`/`relations`** as if tables, but they're columns added by ALTER TABLE (low/medium). Self-induced.
  - `just verify` and `just ci` recipe descriptions missing `deny` step (low)
  - `db.rs "~730 lines"` actually 1072 (low cosmetic)
- ✅ `docs/architecture.md` — **MINOR ISSUES**. Dependency list, WAL/FK, bundled rusqlite, urgency-not-stored, UI embedding, output formats, exit codes all verified. Cross-doc links all resolve. 3 follow-ups:
  - Add `src/util.rs` and `src/agent_docs.rs` to module list (low) — both omitted; CLAUDE.md explicitly calls out `agent_docs.rs` as a key module
  - List `HasUrgency` trait alongside `commands/mod.rs` helpers (low)
  - Mention `skills` helpers in `db.rs` bullet list (low)
- ✅ `docs/security.md` — **MINOR ISSUES**. Bind, token generation/check (`lower(hex(randomblob(24)))`, 24-byte → 48 hex, process-bound lifetime), `--allow-dangerous` 403 contract all verified. 4 follow-ups:
  - Document 1 MiB body size limit + `X-Content-Type-Options: nosniff` (medium) — undocumented DoS mitigation
  - Clarify token-free `/assets/` paths are exact (`app.css`, `app.js`) not a prefix (low) — current wording implies prefix match the server doesn't implement
  - Add explicit "no Origin/Referer check is performed" statement (low) — closes a threat-model gap
  - Cross-link to `ui-api.md` and `architecture.md` (low) — navigation gap
  - Also surfaced: single-threaded blocking listener can stall on slow clients (operational note)

## Quarantine triage notes

(empty)
