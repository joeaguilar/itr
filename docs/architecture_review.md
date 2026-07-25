# `itr` Architecture Review

**Scope:** whole repository (41 `.rs` files, ~20.5k LOC) at commit `04e9f3a`, clean tree.
**Date:** 2026-07-25
**Method:** three independent blind review lanes → orchestrator verification → five-model priority council.

---

## How this review was produced

Three reviewers audited the **same whole-repo scope**, blind to each other's findings:

| Lane | Model | Lens |
|---|---|---|
| A | Sonnet 5 | duplication, dead code, mechanical smells |
| B | Opus 5 | architecture, abstraction seams, latent correctness |
| C | GPT-5.5 (high effort) | skeptic — silent failures, panics, atomicity, API inconsistency |

All three were seeded with the same `kgr` ground truth **and with four verified false-positive orphans**
(`version_shape.rs`, `install.sh`, `uninstall.sh`, `app.js` — all reachable via `include!`/`include_str!`/shipping),
so no lane wasted effort rediscovering noise or reporting deliberate design as a defect.

Findings were then deduplicated into a single dossier, and **the orchestrator independently verified the
highest-stakes claims before ranking** — six of them by reproducing the failure on a real binary. Only then
did a five-seat council (Opus 5, Sonnet 5, Fable 5, GPT-5.5, GPT-5.6-sol) rank priority and severity.

Verification markers used throughout:

- **`[RUNTIME]`** — orchestrator reproduced the failure on a compiled binary. Proven.
- **`[CODE]`** — orchestrator confirmed by reading the source. Confirmed.
- **`[LANE]`** — reported with lane evidence; not independently re-verified.

---

## Verdict

The codebase is well-built in the ways that are usually hard — **no SQL injection, no XSS, no path
traversal, correct DNS-rebinding defence, a 192-bit CSPRNG token compared in constant time, and a
genuinely thoughtful soft-fallback philosophy**. The defects found are almost all of one kind:

> **The same concept is implemented two-to-four times in parallel code paths, and the copies have drifted
> into different observable behavior.**

That single root cause explains the two P0s, most of the P1s, and nearly every "duplication" finding. This
is not a codebase with sloppy code; it is a codebase whose abstractions exist but are **bypassed by half
their would-be callers**. `add` vs `batch add` vs `ui::create_issue`; `update` vs `batch update` vs `bulk
update`; `get::fetch_detail` vs `ui::issue_detail`; four independent renderers for one output grammar.

**Recommendation: fix the two P0s before further feature work.** Both are silent data-destroying defects on
default paths. The council's consensus fix-first order is **B1 → A2 → B3 → A1 → B12**.

---

## Priority summary

| Priority | Count | Meaning |
|---|---|---|
| **P0** | 2 | Silent data loss on a default path. Fix first. |
| **P1** | 18 | Real defect, agent-visible, bounded blast radius. |
| **P2** | 17 | Deferrable — structure, perf, maintainability. |
| **P3** | 4 | Nits. |

The council was **unanimous** on both P0s and unanimous on eleven other calls. Where seats disagreed, the
dissent is recorded with the finding.

---

# P0 — Fix first

### P0-1 · `update` silently clobbers a field the caller never asked to change · `[CODE]`
`src/commands/update.rs:182-205` · sources: opus + gpt-5.5 · **council: P0 unanimous (5/5)**

A typo'd `--priority`/`--kind` on `itr update` **overwrites the issue's current value** with `medium`/`task`.
Every parallel writer keeps the current value:

| Path | `--priority critcal` (typo) → |
|---|---|
| **`itr update 1`** | **overwrites to `medium`** |
| `itr batch update` | keeps current + REVIEW note |
| `itr bulk update` | keeps current + REVIEW note |

`update.rs`'s **own status handler** (lines 165-173) keeps the current value, citing issue #163: *"a typo must
not mutate workflow state the caller never asked to change."* That reasoning was never applied to priority
and kind in the same function.

So `itr update 1 --priority crtical` silently **downgrades a critical issue to medium** — on the single most
used agent-facing command, from one keystroke, with the correct behavior already implemented three times
elsewhere in the codebase.

> **Why the council put this above every crash:** four seats independently argued it was *under*-tiered in the
> dossier. A crash is loud, visible, and destroys nothing. This silently corrupts the field agents schedule on,
> and no one ever finds out. Reachability is one typo on the busiest path.

**Fix:** mirror `update.rs`'s own status handler — push the `REVIEW:` note, write nothing.

---

### P0-2 · `import` destroys collateral rows and orphans issues not in the payload · `[RUNTIME]`
`src/commands/import.rs:56-58` · source: opus · **council: P0 unanimous (5/5)**

`INSERT OR REPLACE` is delete-then-insert, and foreign keys are **ON** with `ON DELETE CASCADE`
(dependencies, notes, events, relations) and `ON DELETE SET NULL` (`parent_id`). Replace is the **default**;
`--merge` is the opt-out. `export | import` is documented as the restore path.

**Reproduced on a real binary.** Re-importing *only* issue 2:

| | before | after |
|---|---|---|
| issue 2 notes | 1 | **0** |
| issue 2 `blocked_by` | `[1]` | **`[]`** |
| **issue 3** `parent_id` | `2` | **`None`** |

**Issue 3 was orphaned and it was never in the import payload** — collateral destruction of an untouched issue.

> **Orchestrator correction to the original finding, in the tool's favor:** import is *not* silent. It does print
> `REVIEW: import replaced 1 existing issue(s)...`. But that warning names only the replacement — it never
> mentions the destroyed note, the destroyed dependency edge, or the child's lost parentage. **The orphaned
> child produced no warning at all.** The finding stands; the "silently" framing needed narrowing.

**Fix:** use `INSERT … ON CONFLICT(id) DO UPDATE SET …` instead of `OR REPLACE`, and name the collateral rows
in the REVIEW note.

---

# P1 — Fix soon

### P1-1 · Skills lowercased on write, compared case-sensitively on filter · `[RUNTIME]`
`src/db.rs:491` (+ `add.rs:82`, `batch.rs:248`, `ui.rs:1085`) · source: opus · council: P1 (4/5, opus said P0)

**Reproduced:** `itr add "x" --skill Rust` stores `["rust"]`. Then:

```
itr list --skill Rust  →  "No matching issues found."
itr list --skill rust  →  found
```

The user cannot find issues by the string they typed. `ListFilter` is shared, so `search`, `ready`, `next`,
`claim`, and the UI all inherit it. Status/priority/kind filters **are** normalized — skills is the one that
isn't. An agent concludes no work exists.

**Fix:** lowercase filter values in `list_issues`/`search` the same way the write path does.

---

### P1-2 · Reachable Unicode panic in search, exit code 101 · `[RUNTIME]`
`src/commands/search.rs:343` · source: gpt-5.5 · council: P1 (4/5, sonnet said P0)

`extract_snippet` computes `match_start` from `text.to_lowercase().find()` — offsets into the **lowercased**
string — then slices the **original** `text`. `to_lowercase()` is not byte-length preserving. The boundary-safety
loops guard `snippet_start`/`snippet_end` but **not** `match_start`/`match_end`, directly under a doc comment
claiming *"UTF-8 safe — always slices on char boundaries."*

**Reproduced:** title `İstanbul index`, then `itr search i`:

```
thread 'main' panicked at src/commands/search.rs:343:24:
byte index 1 is not a char boundary; it is inside 'İ' (bytes 0..2) of `İstanbul index`
EXIT CODE: 101
```

Exit 101 violates the documented contract (`0: success, 1: error`), and dumps a Rust backtrace to stderr.

> **Orchestrator refinement:** the originally-stated repro (`İstanbul` + query `i`) does **not** fire — SQLite's
> `LIKE` is ASCII-only, returns no rows, and the snippet code is never reached. The title needs an
> **ASCII-matchable term as well** (`index`). The mechanism was right; the trigger was narrower than stated.

**Fix:** compute snippets with char indices, or map case-insensitive matches back to valid original boundaries.

---

### P1-3 · `is_blocked` is a declared-supported field that renders nothing in the default format · `[LANE]`
`src/format.rs:683-704` · sources: gpt-5.5 + opus · **council: P1 unanimous (5/5)**

`is_blocked` passes `warn_list_unsupported_fields`, and is present in `IssueSummary`, `PRETTY_LIST_COLS`, and
`oneline_field_value` — but in **neither** compact capability list. So `itr list --fields is_blocked` gives:

| format | result |
|---|---|
| json | correct |
| pretty | correct (`Blk` column) |
| oneline | correct |
| **compact (the default)** | **empty output, exit 0** |

Same gap in `format_issue_detail_compact` and `format_search_compact`. Silent wrong data on request, in the
default format, with a success exit code.

**Fix:** derive all four renderer tables from one field registry so a field cannot exist in three of four.

---

### P1-4 · `preprocess_args` rewrites "getting started" anywhere in argv · `[LANE]`
`src/main.rs:20-37` · source: opus · **council: P1 unanimous (5/5)**

The `position()` scan is unscoped — it never checks that the match is in the subcommand slot. So
`itr note 5 getting started on the parser` silently stores the note body as
`"getting-started on the parser"`. Same for titles, close reasons, and tag values.

> Fable independently confirmed reachability: `note` takes `Vec<String>` and the docs bless unquoted text, so
> this corrupts durable storage from ordinary English prose.

**Fix:** only merge when the pair occupies argv positions 1-2.

---

### P1-5 · UI PATCH silently reopens closed issues · `[CODE]`
`src/commands/ui.rs:983-1009` · sources: **all three lanes** · council: P1 (4/5, gpt-5.5 said P0)

An unrecognized status in a UI PATCH is forced to `"open"`, so `PATCH /api/issues/7 {"status":"finished"}` on a
`done` issue **reopens it** and records a `done → open` event as if deliberate — the exact regression #163 fixed
on the CLI side. It also picks `medium`/`task` fallbacks with **no `REVIEW:` note and no `_needs_review` tag**,
while `create_issue` **30 lines above in the same file** does both.

> Three seats rated this below P0-1 despite the identical bug class, on reachability: the shipped UI's dropdowns
> don't emit unrecognized statuses, so it needs a hand-crafted PATCH. The CLI equivalent needs one typo.

**Fix:** keep `old_issue.status` and append a REVIEW note, matching `update.rs`.

---

### P1-6 · `add_dependency` can create the one "unrecoverable" invariant violation · `[LANE]`
`src/db.rs:685-732` · sources: gpt-5.5 + opus · **council: P1 unanimous (5/5)**

Check-then-act with no transaction: `has_path()` is a plain read, then `INSERT`, unguarded. Two concurrent
`itr depend` processes adding `A→B` and `B→A` can each pass the cycle check and jointly create a cycle — the one
invariant this project designates unrecoverable.

The correct pattern **already exists twelve lines above**: `claim_issue` uses `TransactionBehavior::Immediate`
with a compare-and-swap, and carries a doc comment explaining exactly why the pre-read must happen under the
write lock. `add_dependency` was written without it. `ui.rs:953,1106` likewise use `unchecked_transaction()`
(BEGIN DEFERRED) where `Immediate` is required.

> Opus argued reachability is real rather than theoretical **here specifically**, because this repo's own workflows
> run parallel multi-agent blitzes against one DB.

**Fix:** wrap in an `Immediate` transaction, following `claim_issue`.

---

### P1-7 · `import` is the only mutation path that never normalizes · `[LANE]`
`src/commands/import.rs:56-76` · sources: opus + gpt-5.5 · council: P1 (5/5)

`status`/`priority`/`kind` go straight from JSON into `INSERT OR REPLACE`. A **recognized synonym** —
`"priority":"urgent"`, which `normalize_priority` maps to `critical` on every other path — hits the raw SQL CHECK
constraint: `ERROR: Database error: CHECK constraint failed`, exit 1, and **every item in the file is lost**. The
DB constraint is doing validation duty the soft-fallback layer was built to do.

Related: `import.rs:94` uses `let _ = tx.execute(...)`, silently discarding failed dependency inserts with no count.

**Fix:** run each imported issue through `normalize_*`/`validate_*` with `add.rs`'s soft fallback before insert.

---

### P1-8 · `bulk --dry-run` does not preview the real run · `[LANE]`
`src/commands/bulk.rs:81-104, 199-235` · source: opus · council: P1 (4/5)

`if !dry_run { let tx = …; … }` means dry-run performs no reads, no validation, and cannot populate
`all_unblocked`. `itr bulk close --tag z --dry-run` prints `BULK_CLOSE: 1 issues [1]`; the real run prints that
**plus** `UNBLOCKED:2 "downstream"`. A misleading preview on a destructive verb.

Every batch verb and the other three bulk verbs do it correctly — running the identical path inside a rolled-back
transaction, the pattern `batch.rs:183-187` documents as the contract.

**Fix:** always open the transaction; commit only when `!dry_run`.

---

### P1-9 · Unescaped title can forge records in parseable stdout · `[LANE]`
`src/commands/bulk.rs:485` · source: opus · council: P1 (4/5, fable said P2)

`println!("UNBLOCKED:{} \"{}\"", u.id, u.title)` bypasses `format::escape_quoted_value` — the documented
project-wide encoding that `format::format_unblocked` uses for the *identical* line. A title containing
`a"\nUNBLOCKED:999 "forged` makes one unblocked issue print two `UNBLOCKED:` records, which a parsing agent reads
as real. `format.rs:216-224` explicitly says *"Reuse them for any new output instead of inventing another scheme."*

**Fix:** delete the hand-rolled text branch and call `format::format_unblocked`.

---

### P1-10 · Single-ID paths lose guards the multi-ID paths enforce · `[LANE]`
`close.rs:64-71`, `depend.rs:49`, `relate.rs:63` · source: opus · council: P1 (3/5, fable+sonnet said P2)

Five commands special-case `ids.len() == 1` into a separate implementation, and three of them **lose guards**:

- `itr relate 5 --to 5` **succeeds**, creating a self-relation (the `relations` table has no self-CHECK), while
  `itr relate 5,6 --to 5` correctly refuses with a REVIEW note.
- `itr depend`: the multi path guards `id == on`; the single path hits the raw CHECK constraint and surfaces an
  opaque `DB_ERROR`.
- `itr close 5 --duplicate-of 5` writes the relation **outside any transaction** with no self-check, so a later
  failure leaves it committed against an issue that is not closed.

Whether you pass one ID or two changes the rules.

**Fix:** make the multi path the only path.

---

### P1-11 · `bulk` normalizes filters but never validates them · `[LANE]`
`src/commands/bulk.rs:33-41` · source: opus · council: P1 (3/5, fable+sonnet said P2 — "fails safe")

`itr bulk close --status opne` → `BULK_CLOSE: 0 issues []`, exit 0, no warning. `itr list --status opne` warns.
An agent reads "nothing matched" rather than "your filter was wrong" — on a destructive verb.

> Two seats downgraded this because the failure direction is inert (matches zero, destroys nothing). Recorded as a
> genuine split.

**Fix:** swap in `normalize_*_filters` and print the returned notes.

---

### P1-12 · `batch update`'s REVIEW notes lack the `REVIEW:` prefix · `[CODE]`
`src/commands/batch.rs:554-652` · source: sonnet · **council: P1 unanimous (5/5)**

`run_update_core`'s six soft-fallback notes read `"status '{}' not recognized, kept '{}'..."` — no prefix.
`run_add_core` **in the same file** reads `"REVIEW: priority '{}' not recognized..."`, as does `update.rs`. Any
consumer filtering on the documented `REVIEW:` marker **silently misses every batch-update anomaly**.

All five seats moved this out of the dossier's maintainability tier: it is a contract break, not a style nit.

**Fix:** route all six push sites through the same `"REVIEW: ..."` format used everywhere else.

---

### P1-13 · UI re-implements five command handlers, and the copies have drifted · `[LANE]`
`ui.rs:863-949, 1097-1228, 1385-1406` · source: opus · council: P1 (4/5)

Not duplication — **active drift**. `ui::issue_detail` populates `children` for *every* issue while
`get.rs::fetch_detail` does so only for epics and uses `None` when empty, so **`itr get 5 -f json` and
`GET /api/issues/5` return different JSON for the same row**. `ui::batched_issue_details` is a line-for-line copy
of `get.rs::collect_details`. `ui::resolve_issue` is a verbatim copy of `close.rs::close_issue`.
`ui::list_issue_summaries` loads all issues and filters in Rust, duplicating `db::list_issues(&ListFilter)` whose
filter struct is an exact superset of the query params it parses.

Every close-time or create-time rule added from here lands in one of two places.

**Fix:** make `fetch_detail`/`collect_details`/`close_issue` `pub(crate)` and have UI routes call the CLI handlers.

---

### P1-14 · `add` and `batch add` disagree on identical input · `[LANE]`
`add.rs:98-173` vs `batch.rs:205-265` · sources: sonnet + opus · **council: P1 unanimous (5/5)**

~60 lines copy-pasted — including comment text and the `#167` marker — and **already drifted**:

```
echo '{"title":"x","blocked_by":[999]}' | itr add --stdin-json
  → exit 1, nothing created

same object via itr batch add
  → exit 0, issue created, REVIEW note, _needs_review tag
```

The module coupling is already inverted: `add.rs` imports `parse_add_item` **from** `batch.rs`; `batch.rs` imports
`persist_list_field` **from** `update.rs`. `ui::create_issue` is a third copy with no transaction and no parent check.

**Fix:** extract one `create_issue(tx, req)` into a neutral module that add, batch, import, and ui all call.

---

### P1-15 · `oneline` promises a stable field count and doesn't deliver · `[LANE]`
`src/format.rs:640-661` · source: opus · council: P1 (4/5)

The doc comment claims *"a stable tab-separated field count"*, but the assignee cell is **conditionally appended** —
5 fields when `assigned_to` is empty, 6 when not. The file's own tests assert both. `itr list -f oneline | cut -f6`
cannot address a column in a format whose entire purpose is positional parsing.

Separately, `format_unblocked`'s Json arm bypasses `apply_fields_filter` entirely, so `--fields id -f json` emits
the full object.

**Fix:** always emit the assignee cell; route `format_unblocked` through `apply_fields_filter`.

---

### P1-16 · Batch compact drops `UNBLOCKED` notifications on the review branch · `[LANE]`
`src/format.rs:1726` · source: gpt-5.5 · council: P1 (4/5)

The `"review"` formatter branch prints only notes; the `"ok"` branch prints `unblocked`. So a batch update that
both sets a terminal status *and* produces review notes **never announces the issue it unblocked** — the agent
never learns work became ready.

**Fix:** render `item.unblocked` independently of outcome.

---

### P1-17 · Summary construction hides dependency query failures · `[LANE]`
`src/commands/mod.rs:65` · source: gpt-5.5 · council: P1 (3/5, opus+fable said P2)

`build_issue_summary_owned` uses `unwrap_or_default` for blockers/blocks and `unwrap_or(false)` for `is_blocked`.
If a dependency query fails, `itr list` reports `blocked_by=[] is_blocked=false` instead of erroring — an agent then
works a blocked issue believing it is ready.

> Split on reachability: it needs a mid-command query failure on a healthy local SQLite file.

**Fix:** make summary construction fallible and propagate the error.

---

### P1-18 · Schema is never reconciled after `init` — forward-migration gap · `[RUNTIME]`
`src/db.rs:179-197` · source: opus · council: P1 (3/5, fable+sonnet said P2)

`open_db` runs only `migrate_current_schema`, which is **exactly four hand-written migrations** (skills column,
assigned_to column, events table, relations table). It never executes the idempotent `SCHEMA` const, which alone
defines 11 indexes and the `updated_at` trigger.

**Reproduced:** drop `trg_issues_updated_at` → `itr list` does not restore it → `itr doctor` reports
`DOCTOR: All clean` (exit 0) → `updated_at` is **frozen forever**, silently corrupting `--sort updated`, the urgency
`age` component, and every staleness report.

> **The corruption framing understates this, and the council's demotion partly rests on it.** The dropped-trigger repro
> needs tampering — but the **forward-migration gap needs nothing at all**: *any index or trigger added to `SCHEMA` in a
> future release will never reach a single existing database.* Every future schema addition silently depends on someone
> remembering a hand-written `migrate_add_*`; putting it in `SCHEMA` — the natural place — is the failure mode.
> **Orchestrator verified this structurally.** Two seats ranked A4 before this reframing was available to them.

**Fix:** run `conn.execute_batch(SCHEMA)` from `open_db`, and have `doctor` assert the expected `sqlite_master` object set.

---

# P2 — Follow-up

| # | Finding | Location | Verified | Council |
|---|---|---|---|---|
| P2-1 | **`parse_json_array` converts corrupt JSON to `[]`, then the next write persists it** — permanent loss. Reproduced: `["keepme", broken` → `itr get` reports `tags: []` with no error → `--add-tag foo` → column becomes `["foo"]`, `keepme` gone. **Requires pre-existing corruption** (dangerous-SQL, torn write, manual edit) — a *corruption amplifier*, not a spontaneous fault. All 5 seats demoted it from blocker on exactly this reachability. | `db.rs:338-340` | `[RUNTIME]` | P1→P2 split |
| P2-2 | Canonical status/priority/kind vocabulary restated in **~8 independent authorities** (SQL CHECKs, `normalize_*`, `validate_*`, six literal `"Valid: ..."` strings, urgency coefficients, `priority_ord`, stats zero-init, help text) with no shared constant and no test tying them together. `CANONICAL_*` consts exist **only inside `#[cfg(test)]`**. | `normalize.rs`, `db.rs`, `urgency.rs`, `list.rs`, `stats.rs`, `format.rs`, `ui.rs`, `cli.rs` | `[LANE]` | P2 (4/5) |
| P2-3 | N+1 queries: one `IssueSummary` costs ~6 round-trips with `is_blocked` computed **twice**, and `--limit` is applied **after** every summary is built — `itr list --limit 5` on 1000 issues issues ~6000 queries. `search.rs` fetches the same row up to 4×. | `mod.rs:59-89`, `urgency.rs:283-334`, `search.rs:99-122` | `[LANE]` | P2 (5/5) |
| P2-4 | `record_event` has no `agent` parameter and always reads `ITR_AGENT` from env, so an explicitly-passed agent never reaches the audit log: `add_note(conn,id,c,"alice")` stores `notes.agent="alice"` but the event stores `""`. `itr log --agent alice` misses them. | `db.rs:1137-1151` | `[LANE]` | P2 (4/5) |
| P2-5 | `add`/`depend`/`relate`/`import`/`doctor --fix` record **no audit events** while every other mutation does — `itr log <new-id>` is empty until first update. `remove_blocker_edges` (called on every close path) deletes edges with no event, while `remove_dependency` records one. | `db.rs:287-314`, `db.rs:858-864` | `[LANE]` | P2 (4/5) |
| P2-6 | `--allow-dangerous` SQL runs **either only the first statement or all of them**, chosen by whether statement #1 returns columns (`prepare` vs `execute_batch`). `SELECT 1; UPDATE issues SET status='wontfix';` reports `changes: 0` and mutates nothing; reorder and everything runs. No warning either way. Also the one UI mutation path with no transaction. | `ui.rs:790-804` | `[LANE]` | P2 (4/5) |
| P2-7 | `doctor` calls `std::process::exit(1)` from inside a `Result`-returning handler and hand-rolls its own JSON error envelope, bypassing `ItrError`/`handle_error`. `main.rs:42-48` does the same for an invalid `--format` and **cannot emit the JSON envelope even in json mode** — while `--fields`, two lines below, warns and continues. | `doctor.rs:46-56`, `main.rs:42-48` | `[LANE]` | P2 (3/5) |
| P2-8 | `update.rs` re-implements the parent existence + cycle guard that `db::update_issue_parent` already performs, with a **byte-identical** error message. The duplicate runs first, so tightening the db-layer guard — whose doc comment says it exists so every caller is protected — silently has no effect on `itr update`. | `update.rs:331-342` | `[LANE]` | P2 (5/5) |
| P2-9 | Three near-identical ~28-line files/tags/skills replace-then-edit-then-persist blocks, differing only in field name and case-folding. | `update.rs:230-319` | `[LANE]` | P2 (5/5) |
| P2-10 | `search_issue_ids` and `search_note_issue_ids` duplicate the entire status/priority/kind filter block **and** the params/prepare/query_map tail byte-for-byte. | `db.rs:989-1066` | `[LANE]` | P2 (5/5) |
| P2-11 | `--quiet` is defined, **advertised in `--help`** ("Suppress non-essential output"), and never read — exactly one hit in `src/`. Same "silently swallow input" class that #197 fixed for `--fields`. | `cli.rs:19` | `[RUNTIME]` | P2 (3/5) |
| P2-12 | `fts_rebuild` returns `InvalidValue` for "SQLite lacks FTS5" — a system capability issue. CLAUDE.md names this exact anti-pattern. `close.rs:82-88` and `relate.rs:90-94` do the same when all requested issues were missing (`NotFound` is truthful). | `db.rs:1477-1490` | `[LANE]` | P2 (3/5) |
| P2-13 | UI accept loop is strictly serial with a per-**read** (not per-request) timeout, so any web page can wedge the UI without knowing the token by drip-feeding bytes — each byte resets `SO_RCVTIMEO`. The connection is consumed *before* `require_token` runs. Availability-only under the localhost threat model. | `ui.rs:188-225` | `[LANE]` | P2 (4/5) |
| P2-14 | `add_note` inserts the note then records its event non-atomically; UI create inserts the issue then adds dependencies outside a transaction, so a dependency failure leaves a created issue behind. | `db.rs:877`, `ui.rs:927` | `[LANE]` | P2 (3/5) |
| P2-15 | `bulk` filter grammar accepts only **one** value per filter (`Option<String>`) while `list`/`ListFilter` support `Vec` plus `--tag-any` OR logic. The capability gap is purely in bulk's plumbing. | `bulk.rs:14-59` | `[LANE]` | P2 (5/5) |
| P2-16 | `bulk relate`/`depend`/`note` each hand-roll their own JSON/text output block instead of the shared `BulkResult` + `print_result` used by `close`/`update` in the same file, producing a different field set per verb. | `bulk.rs:293-463` | `[LANE]` | P2 (4/5) |
| P2-17 | UI create hard-errors on a missing parent (raw SQLite FK message as a 500) while CLI `add`/`batch add` soft-fall-back to parentless creation with a REVIEW note. | `ui.rs:937` | `[LANE]` | P2 (4/5) |

---

# P3 — Nits

| # | Finding | Location | Council |
|---|---|---|---|
| P3-1 | `pub fn all_notes` has **zero callers** — confirmed dead by both `kgr dead` and `rg`. `[CODE]` | `db.rs:1126` | P3 (5/5) |
| P3-2 | `validate_relation_type` reimplemented in `ui.rs` instead of calling the existing `pub(crate)` one in `relate.rs` (which `bulk.rs` already imports). Adding a 4th relation type needs 3 edits. `[CODE]` | `ui.rs:1492` vs `relate.rs:8` | P2/P3 |
| P3-3 | An identical `fn seed(conn, title)` test helper with the same 10-positional-arg insert is copy-pasted into **≥7** `#[cfg(test)]` modules. | 7 test modules | P3 (5/5) |
| P3-4 | The "positional value vs flag, flag wins, warn on conflict" merge logic is duplicated between `Commands::Add` (title) and `Commands::Close` (reason). | `main.rs:176-186` vs `304-314` | P3 (5/5) |

---

## Cross-cutting themes

**1. Parallel writers that disagree.** The dominant defect class. `add`/`batch add`/`ui::create_issue`;
`update`/`batch update`/`bulk update`; single-ID vs multi-ID paths; `get::fetch_detail` vs `ui::issue_detail`. In
every case an abstraction exists and half the callers bypass it. This produced **both P0s** and eight P1s. The
highest-leverage structural fix in the codebase is extracting one issue-write path and one issue-read path that
CLI, batch, bulk, import, and UI all call.

**2. One output grammar, four independent renderers.** `format.rs` implements compact, json, pretty, and oneline as
four hand-maintained implementations — the list renderer table-driven by consts, the others hand-written `if on(...)`
chains. A field added to one never reaches the others (P1-3, P1-15, P1-16, P2-2). The project's own memory records
shipping this exact bug before (ID:216). It will ship again until the four renderers derive from one field registry.

**3. Soft fallback applied unevenly.** The philosophy is sound and mostly well-executed, but the *edges* disagree:
`import` never normalizes at all; `bulk` normalizes but never validates; `batch update` omits the `REVIEW:` marker;
`ui` PATCH swallows silently; `update` defaults where its siblings keep. The philosophy needs a shared
implementation, not a shared description.

**4. Check-then-act without transactions.** `add_dependency`, `add_note`+event, UI create+dependencies, and
`close --duplicate-of` all mutate across unguarded boundaries — while `claim_issue` twelve lines away demonstrates
the correct `Immediate`-transaction pattern with a doc comment explaining why.

---

## Checked and explicitly NOT defects

Recorded so they are not re-audited:

- **No SQL injection.** The only dynamic SQL (`db.rs:542`) is guarded by a `VALID_COLUMNS` allowlist; `append_in_clause` interpolates column names from literals only.
- **No XSS.** Every user-controlled interpolation in `app.js` goes through `escapeHtml`.
- **No path traversal.** UI assets are a fixed `match` on exact paths with no filesystem access.
- **DNS rebinding is correctly blocked** by the Host check; the session token is 192 bits from SQLite's CSPRNG, compared in constant time.
- **No UTF-8 panic in the formatter** — `truncate_with_ellipsis`, `display_width`, and `pad_display` all iterate `chars()`.
- **`--allow-dangerous` is not reachable without the flag.**
- **DOT output escaping is correct**, including truncate-before-escape ordering.
- The `kgr`-reported cycle across `src/commands/*.rs` is **mod.rs re-exports**, not a real import cycle.
- `version_shape.rs`, `install.sh`, `uninstall.sh`, `app.js` are flagged as orphans by `kgr` but are **not dead** — reached via `include!`, `include_str!`, or shipping.

---

## Appendix — council record

Five seats independently ranked all 39 deduplicated findings (presented here as 41 entries, since two
dossier items covered two distinct defects each). Consensus fix-first order by Borda count:

| Rank | Finding | Points | Seats placing it top-5 |
|---|---|---|---|
| 1 | **P0-1** (`update` clobber) | 23 | 5/5 |
| 2 | **P0-2** (`import` cascade) | 22 | 5/5 |
| 3 | **P1-1** (skills case filter) | 12 | 5/5 |
| 4 | **P1-2** (Unicode panic) | 8 | 3/5 |
| 5 | **P1-3** (`is_blocked` compact) | 4 | 3/5 |

**Where the council overruled the initial tiering.** The dossier grouped four findings as blockers (Unicode panic,
import cascade, `parse_json_array`, schema reconciliation). All five seats rejected three of them on reachability —
each requires pre-existing corruption, tampering, or an exotic input — and all five promoted the `update` clobber
*into* P0 from the second tier. The council's reasoning, which this document adopts: **a loud crash that destroys
nothing ranks below a silent one-keystroke clobber of a field agents schedule on.**

**Recorded genuine splits:** P1-11 (`bulk` filter validation — two seats argue it fails safe), P1-17 (summary error
suppression — reachability), P1-18 (schema reconciliation — two seats ranked it before the forward-migration
reframing was available), P2-1 (`parse_json_array` — severity high, reachability low).

**Notes on lane quality.** Corroboration across independent lanes was strongest on `ui.rs` PATCH (all three lanes),
`all_notes` (two), `validate_relation_type` (three), `is_blocked` (two), `parse_json_array` (two), and
`add_dependency` (two). Two of the five highest-ranked findings came from a **single** lane and survived independent
verification — corroboration ranked evidence here, it did not define it.
