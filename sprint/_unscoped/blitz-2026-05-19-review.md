# Code Review: Blitz `code-review-2026-05-19`

Reviewers convened: Verification Auditor, Strict Senior Engineer, Security Reviewer, and the Comedy Code Roaster. Subject: the uncommitted changes produced by the 9-issue blitz documented at `sprint/_unscoped/blitz-2026-05-19T03-36-23Z.md`.

## Overall ratings

| Dimension | Score | One-line |
|---|---|---|
| Correctness vs. issue ACs | **9 / 10** | 9/9 issues genuinely fixed; one (#113) ships a behavior that contradicts project policy. |
| Code quality | **7 / 10** | Real fixes, occasional sloppy shape — tuple returns, loop-invariant rechecks, O(n²) dedupes. |
| Test coverage | **6 / 10** | Suite grew 283 → 308, but several new tests pass-when-broken; negative cases thin. |
| Security posture | **8 / 10** | One Medium in `install.ps1`; everything else clean. |
| Process integrity | **7 / 10** | No false closures, no quietly-filed cover-ups; but the same agents graded their own homework. |
| **Overall** | **7.4 / 10** | Solid blitz. Ship with two targeted follow-ups (MED-1, MED-2) and a `bulk.rs` cleanup. |

## Verification summary

All 9 closed issues genuinely fixed. Acceptance criteria met by real code changes in the right layer, each backed by integration tests. The blitz plan's `Interventions` section honestly disclosed the Wave 1 sandbox/localhost failure and the early pull-forward of #117 — that pre-emptive transparency matches what the code actually shows.

| Issue | Verdict |
|---|---|
| #79 release smoke tests | GENUINELY FIXED |
| #80 install.ps1 checksum | GENUINELY FIXED (but see SEC-1) |
| #81 BREAKING CHANGE detection | GENUINELY FIXED |
| #112 FTS + notes merge | GENUINELY FIXED |
| #113 ready non-terminal | GENUINELY FIXED (but see MED-2) |
| #114 terminal-status edge cleanup | GENUINELY FIXED |
| #115 atomic add+blocked_by | GENUINELY FIXED |
| #116 fresh-init schema parity | GENUINELY FIXED (minor FTS gap, see LOW-1) |
| #117 binary path normalization | GENUINELY FIXED |

No issues were quietly filed during the blitz as cover-ups. Open follow-ups (#118–#133, #135) belong to the prior doc-sweep commit, not this blitz.

## Issues found (ranked)

### MED-1 — `search.rs:33` unconditional LIKE table scan on every query
`let like_ids = db::search_issue_ids(...)` now runs at the top of `run()` regardless of FTS state. Previously this was in the FTS-empty branch. Every search now pays both an FTS query AND a multi-column LIKE scan. For a small SQLite DB this is fine; for a scaled tracker it's a performance regression. Fix options:
- Make `like_ids` lazy — only compute when FTS is unavailable OR for the merge step.
- Better: replace with a notes-only `SELECT DISTINCT issue_id FROM notes WHERE content LIKE …`, since FTS already covers title/context/acceptance/tags/files/skills/close_reason. The merge then has no dedup work to do because the two result sets are disjoint by construction.

### MED-2 — `ready.rs:33-35` silently swallows terminal-status filters
`itr ready --status done` returns `[]` with no warning. This directly contradicts CLAUDE.md's documented Soft Fallbacks Philosophy: *"Never silently swallow input — if a flag consumes a value the user likely intended for another argument, detect and warn."* The integration test at `tests/integration.sh:432-435` *codifies* the silent behavior, locking in the bug. Fix: emit a `REVIEW:` note to stderr when an explicit terminal status is dropped, and update the test to assert the note is emitted.

### SEC-1 (Medium) — `install.ps1:131-138` broadened catch swallows non-404 failures
The new bare `catch` suppresses every terminating error from the `.sha256` download — not just HTTP 404, but PS7 `HttpResponseException`, TLS handshake aborts, disk-full while writing `$sumPath`, etc. The installer then proceeds with `$hasChecksum = $false` and installs unverified. Threat model: an attacker who can selectively disrupt the smaller `.sha256` asset (proxy/CDN edge that 5xx's, partial availability) drives the installer onto the unverified path. Fix: narrow to known-benign exceptions (`HttpResponseException` with `StatusCode -eq 404`), or at minimum surface `$_.Exception.Message` in the warning so operators notice non-missing-file failures.

### LOW-1 — Fresh init still lacks the FTS virtual table
`db.rs:206-211` `init_db` runs `SCHEMA` + `migrate_current_schema` but never `try_create_fts`. An old DB that has gone through `open_db` *does* have `issues_fts`. The fresh-init schema test (`tests/integration.sh:121-159`) doesn't assert FTS presence, so it won't catch the drift. The issue's claim "init == init+migrate" is partly false. Fix: call `try_create_fts(&conn)` from `init_db`, and assert it in the test.

### LOW-2 — `update.rs` terminal-status cleanup is not transactional
`update.rs:198-204` runs `get_newly_unblocked` and `remove_blocker_edges` on the raw `conn`, not inside a transaction with the status write at line 51. If `remove_blocker_edges` fails after the status flip, edges are partially deleted and the reported `unblocked` list is wrong. `add.rs:180-206` shows the right pattern. `batch.rs` and `bulk.rs` are transactional; only the single-issue `update.rs` is the odd one out. The blitz harmonized the *call site* across three paths but not the *transactional structure*.

### LOW-3 — Asymmetric `--blocked-by` failure modes
`add.rs:148-153` soft-falls back for non-numeric tokens (`abc` → warn + continue) but `add.rs:202-203` still hard-fails (rolls back the whole issue) for missing numeric IDs (`999` → error). The asymmetry is undocumented. Either document it in the REVIEW note, or pre-validate all blocker IDs with `issue_exists` and surface ALL bad IDs in one note.

### LOW-4 — `bulk.rs:185-196` O(n²) dedupe and loop-invariant recheck
`any(|u: &UnblockedIssue| u.id == uid)` linear scan inside the loop. Use a `HashSet<i64>`. Also `terminal_status.is_some()` is checked every iteration but is loop-invariant — branch outside the loop.

### LOW-5 — `auto-version.yml` false-positive surface
`grep -qE '^BREAKING CHANGE:'` scans the full commit (subject + body). A revert commit body like `Reverts: "feat!: foo" BREAKING CHANGE: bar` would trigger a major bump. Acceptable in practice but worth a comment in the workflow.

### LOW-6 — Terminal-status string is hard-coded in four files
`close.rs`, `update.rs:52`, `batch.rs:400`, and `bulk.rs:158` each independently check `"done"` / `"wontfix"`. The next status added (e.g. `"abandoned"`) requires four edits. Extract `is_terminal_status(s) -> bool` into `normalize.rs` or `db.rs`.

## Test gaps

- **`tests/integration.sh:1158`** — `next((... for item in d if item['id'] == 3), False)` passes trivially when the merge is broken (returns `False`, never asserts presence). Add `assert 3 in [i['id'] for i in d]` first.
- **No partial-rollback test** for `--blocked-by 1,2,999` (1 and 2 valid). The rollback claim is only verified for a single bad ID.
- **No cycle-rollback test** for `add --blocked-by` triggering `CycleDetected`.
- **No FTS-presence assertion** in fresh-init test (masks LOW-1).
- **Smoke mode is thin** — only `version`, `init`, `add`, `ready`. A binary that segfaults on `search` / `update` / `close` would pass the smoke gate. Consider adding `search` and `close` to smoke.
- **Loose `assert_contains`** for schema strings — `"assigned_to"` matches anywhere, doesn't verify column-on-issues. The deep Python check at lines 121-159 is the right pattern; the shallow asserts are filler.

## Code smells (non-blocking)

- `add.rs` `parse_blocked_by_tokens` returns a 2-tuple of `Vec`s — should be a small struct.
- `search.rs:71-75` `.contains` on a `Vec` for dedupe — use a `HashSet`.
- `db.rs` SCHEMA const duplicates migration `CREATE TABLE` blocks for `events`/`relations` — high drift risk.
- `bulk.rs:166` status is normalized twice per loop iteration.
- `install.ps1` `.NOTES` block describes *manual* verification, not automated tests. Aspirational documentation.

## Concerns about the blitz process

1. **The verify gate was the correctness oracle.** "283 → 308 tests" was treated as proof of correctness, but several new tests are wired such that they would pass when the behavior is broken. Same agent grading own homework.
2. **#113 silently swallows user input** — a direct violation of the project's own documented philosophy. Should have been caught in planning, not implementation.
3. **#114 harmonized the call site but not the transactional structure** across `update.rs` / `batch.rs` / `bulk.rs`.
4. **#80 "verification" is `.NOTES` documentation telling a human to test manually** — that is not verification. The PS5/PS7 divergence is the whole point of the fix; it deserves at least one Pester test or a CI matrix line.
5. **No cross-wave interaction tests.** Wave 1 changed schema, Wave 4 changed FTS semantics, Wave 2 changed insert atomicity. Nothing verifies the three play well together (e.g., add+blocked_by+cycle+rollback on a fresh-init DB, then assert FTS state).

## What went well

- Real bugs, real fixes, in the right layers.
- `add.rs` atomicity work is textbook — `unchecked_transaction` + drop-on-error + manual `commit()`.
- The fresh-init schema work (#116) genuinely closes the gap where new DBs differed from migrated ones (FTS asymmetry aside).
- `install.ps1` correctly catches the PS5/PS7 exception-type divergence — the real underlying bug, not just the symptom.
- The release smoke gate is overdue and minimal in the right way.
- `bulk.rs` now actually populates the `unblocked` response field instead of returning `vec![]` — a quiet bonus fix.
- The blitz plan's `Interventions` section disclosed real friction (Wave 1 gate failure, pull-forward of #117) instead of papering it over.

## Recommendation

**Ship**, with three follow-ups filed before the next release tag:
1. **MED-2 / blocking** — `ready.rs` should emit a REVIEW note for terminal-status filters; the codifying test should be inverted.
2. **SEC-1 / pre-release** — Narrow the `install.ps1` catch or surface the exception message; tag this before cutting v1.
3. **MED-1 / next sprint** — Make `like_ids` lazy in `search.rs` and rewrite as a notes-only LIKE.

LOW-1 through LOW-6 are technical debt — file as a small follow-up sprint, don't block the release.

---

### Comedy Code Roaster's Yelp review (verbatim)

> "Came in for a code review, left with 308 passing integration tests and mild emotional damage. The diff is mostly good — atomic transactions where there should be atomic transactions, real bugs fixed with real one-liners, and a release pipeline that finally checks if its binary boots. Lost a star for the `SCHEMA` const that duplicates the migrations, the O(n²) dedup in `bulk.rs`, the misleading comment in `search.rs`, and a `parse_blocked_by_tokens` helper that returns a tuple like it's allergic to structs. Lost zero stars for the `\036` byte in the YAML, because frankly I respect the audacity. Would blitz again. Ask for table 9, near the `unchecked_transaction`."
>
> — 4 stars
