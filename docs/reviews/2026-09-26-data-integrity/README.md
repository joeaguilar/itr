# Data-integrity review — 2026-09-26

A review of every path that moves data into or out of `.itr.db`: import,
export, `add` and the other write paths, the SQL they run, and the schema
migration process. Four read-only reviewers reproduced each finding against
v3.3.1 (`2e96f5d`); fixes landed on `main` from `2c20353` to `9551ccd`.

| Report | Scope |
|--------|-------|
| [import-export.md](import-export.md) | `import` / `export` validation, round-trip fidelity (IE-1…IE-16) |
| [write-paths.md](write-paths.md) | `add`, `batch`, `bulk`, `update`, notes, UI API; create-path parity table (AW-1…AW-17) |
| [sql-queries.md](sql-queries.md) | Every SQL statement: injection, column order, transactions, NULLs, ordering (SQ-1…SQ-14) |
| [migration-history.md](migration-history.md) | Git archaeology of every schema change, open flow, old-release upgrade experiments |

The line numbers in the reports refer to the reviewed revision, not to `main`.

## What changed

- **One definition of clean input.** `src/sanitize.rs` defines a clean
  title, text, list, skill list, assignee, timestamp, and relation type.
  `db::insert_issue`, `db::update_issue_field`, `db::add_note`, and
  `db::update_note` apply it, so no write path can store a shape the others
  reject (`376a164`).
- **Reads never silently lose data.** Malformed list cells are salvaged with
  a `REVIEW:` note instead of read back as `[]`; invalid UTF-8 or BLOB cells
  decode lossily instead of aborting `list` / `export` (`376a164`).
- **Import validates like `add`.** Enum normalization, cleaned fields,
  timestamp normalization, ID range, duplicate IDs, unknown keys, cycles,
  relation scope, and line-numbered skips for bad records (`8db2128`,
  `9551ccd`). See [Import Validation](../../backup-import-export.md#import-validation).
- **Export is one snapshot** (`8db2128`).
- **Writes take the lock up front.** Every read-then-write mutation uses
  `BEGIN IMMEDIATE` (`db::write_tx` / `db::with_write_tx`) (`2c20353`,
  `6b1c618`).
- **The UI API follows the CLI's rules.** Creation goes through
  `add::execute`, writes are atomic, and malformed input is rejected
  (`dc5a3f0`, `e7b0847`).
- **Schema reconcile on open, plus an upgrade test from old releases**
  (`7a080d7`, `0536ad6`, `e46f322`). The migration process is documented in
  [docs/migrations.md](../../migrations.md) (`6c12554`).

## Disposition

| Finding | Status |
|---------|--------|
| IE-1 i64::MAX id breaks `add` | Fixed `8db2128` |
| IE-2 `OR IGNORE` hides CHECK failures | Fixed `8db2128` |
| IE-3 duplicate ids merge | Fixed `8db2128` |
| IE-4 unknown keys dropped | Fixed `8db2128`, `9551ccd` |
| IE-5 all-or-nothing, wrong line numbers | Fixed `8db2128` |
| IE-6 timestamps unvalidated | Fixed `8db2128` |
| IE-7 list fields not normalized | Fixed `376a164`, `8db2128` |
| IE-8 foreign relations in a bundle | Fixed `8db2128` |
| IE-9 round-trip ordering / dependency `created_at` | Ordering fixed `8db2128`, `86ac8fe`; `created_at` tracked in #287 |
| IE-10 one bad cell aborts export | Fixed `376a164` |
| IE-11 unknown export format | Fixed `8db2128` |
| IE-12 BOM / single object | Fixed `8db2128` |
| IE-13 closed blockers after import | Fixed `8db2128` |
| IE-14 config not exported | Documented (help + backup guide) |
| IE-15 / AW-14 control characters | New writes fixed `376a164`; legacy rows tracked in #286 |
| IE-16 DB errors read as "dangling" | Fixed `8db2128` |
| AW-1 empty titles | Fixed `376a164` |
| AW-2 list cleaning differs by path | Fixed `376a164` |
| AW-3 re-close differs by path | #278 |
| AW-4 UI bulk resolve not atomic | Fixed `dc5a3f0` |
| AW-5 UI non-numeric parent | Fixed `e7b0847` |
| AW-6 UI create drops keys | Fixed `dc5a3f0` |
| AW-7 batch `@self` aborts batch | #282 |
| AW-8 empty notes | Fixed `376a164`, `dc5a3f0` |
| AW-9 stale `close_reason` on reopen | #279 |
| AW-10 enum normalizers do not trim | Fixed `376a164` |
| AW-11 relation types / symmetric links | #280 (import side fixed `8db2128`) |
| AW-12 assignee not trimmed | Fixed `376a164` |
| AW-13 / SQ-13 `updated_at` semantics | #281 |
| AW-15 / AW-16 token and comma parsing | #283 |
| AW-17 UI PATCH ignores wrong types | Fixed `dc5a3f0` |
| SQ-1 deferred transactions → "database is locked" | Fixed `2c20353`, `6b1c618` |
| SQ-2 depend race creates cycles | Fixed `6b1c618` |
| SQ-3 UI stats blocked count | Fixed `dc5a3f0` |
| SQ-4 UI PATCH status parity | Fixed `dc5a3f0` |
| SQ-5 skill filter case | Fixed `8e2c697` |
| SQ-6 FTS vs LIKE semantics | #284 |
| SQ-7 `log --since` string compare | Fixed `c165dea` |
| SQ-8 blocked/ready definitions | Fixed `0dea1b4`, `dc5a3f0` |
| SQ-9 nondeterministic ordering | Fixed `86ac8fe` |
| SQ-10 closed blocker accepted by depend | #285 |
| SQ-11 dangerous-SQL `changes` count | Fixed `dc5a3f0` |
| SQ-12 non-atomic mutations | Fixed `6b1c618`, `dc5a3f0`; claim recheck in #285 |
| SQ-14 redundant index | Left as is: dropping an index needs a migration and gains nothing measurable |
| Known #222 #223 #224 #228 #229 #230 #234 #240 #241 #242 #255 #258 #269 #270 #272 #274 | Closed with commit references |
| Migration-review follow-ups | #286 (doctor), #287 (export version), #288 (UI generation), #289 (CHANGELOG) |
