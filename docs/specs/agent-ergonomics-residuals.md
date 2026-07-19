# spec: agent-ergonomics residuals — multi-ID mutation, bulk relate/depend/note, batch-add dry-run, fields-everywhere

**Status:** implemented — shipped in `68e1a53` (2026-07-02, "feat: multi-ID mutation, bulk verbs, batch dry-run, fields everywhere"), with the batch dry-run precursor in `ae85d40` (2026-03-07). Retained as a design record; see `docs/command-contracts.md` for current behavior.
**Originally proposed:** validated against installed `itr v2.10.2-1-g63b6bac`, 2026-07-02; backlog scanned — 205 issues, 0 open, none covering these
**Origin:** transcript mining across ~86 Claude Code sessions (`werkit/claude-reflection-notes.md` finding #1): ~130 `itr … -f json | python3 -c "json.load…"` reformatting calls and ~110 `for id in $(itr list … | jq -r '.[].id')` loops across ≥7 projects.

## What the audit found already fixed (no work — listed so this spec isn't re-litigated)

The worst mined patterns are solved in the current version; the loops in old transcripts predate these features:
- `itr bulk close --tag X --reason "…" --dry-run` and `itr bulk update --set-status … <filters>` (src/commands/bulk.rs) already replace the close-loop ritual.
- `itr batch close|update|note` (JSON on stdin), with `--dry-run` on close/update.
- `itr get 1,2,3` / `itr show 1 2 3` multi-ID (#136).
- Global `--fields <csv>` works for `-f json` and `-f compact`; `batch add` does per-item soft-fallback with `REVIEW:` notes instead of all-or-nothing rejection (#164, #150).

What follows are the **residual** gaps, each still forcing a shell loop or a python pipe today.

---

## P1 — multi-ID on mutating verbs: `close`, `note`, `relate`, `depend`, `claim`

Today `close/note/relate/depend` take a single scalar `id: i64` (cli.rs:257, 279, 308, 516) while `get/show` already accept `num_args=1..` with comma-splitting. Agents therefore still loop for anything id-list-shaped that isn't expressible as a filter:

```sh
# today (mined shape — red:ff82ea80, Harness:6c8ce498)
for id in 124 125 126 127 128 129 130 131 132; do itr relate "$id" --to 53 --type related; done

# proposed
itr relate 124-132 --to 53 --type related
itr close 12,14,17 "fixed in a1b2c3d"
itr note 55 56 57 --agent fable-review "verified end-to-end"
```

Semantics:
- Accept the same ID syntax `get` uses (space and comma separated), **plus inclusive ranges `A-B`** (also add ranges to `get/show` for symmetry).
- One transaction per invocation; per-ID soft-fallback consistent with `batch` behavior: a missing ID emits `REVIEW: id 126 not found; skipped` and the rest proceed; exit 0 if ≥1 succeeded, 1 if none did.
- Every mutation records its own audit event (the #35 lesson).
- `claim`: accept multiple IDs only with an explicit `--force-multi` guard, or leave single — claiming is intentionally one-at-a-time; decide at implementation and document.

## P2 — filter-based `bulk relate` / `bulk depend` / `bulk note`

`bulk` today covers close and update only. The mined loops that remain unexpressible:

```sh
# today
for id in $(itr list --tag sprint-9 -f json --fields id | jq -r '.[].id'); do itr depend "$id" --on 200; done

# proposed (same filter grammar as bulk close: --status/--priority/--kind/--tag/--skill/--assigned-to)
itr bulk depend --tag sprint-9 --on 200 --dry-run
itr bulk relate --kind bug --status open --to 53 --type related
itr bulk note --assigned-to blitz-3 "wave 2 verified" --agent scrum
```

- `--dry-run` mandatory-supported on all three (prints the would-be edges/notes, like bulk close).
- Self-edges skipped with `REVIEW:` (an issue matching the filter that equals `--on`/`--to`).
- Cycle-creating `depend` edges follow whatever validation single `depend` does today — same code path, not a parallel one.

## P3 — `batch add --dry-run` (and `batch note --dry-run`)

`batch close/update` have `--dry-run` (#19); `batch add`/`batch note` don't. The mined arrays-vs-strings churn (TimelineClock:c24ba5e7 rewriting `sprint1-stories.json`) is *mitigated* by soft-fallback now, but a payload author still can't validate without mutating:

```sh
# proposed
itr batch add --dry-run < sprint1-stories.json
# → per-item verdict, nothing written:
#   [1] OK       "Story: pan gesture on chart"      (tags:2 files:1 blocked_by:[@0→err: no prior item])
#   [1] REVIEW   unknown key "file" — did you mean "files"?
#   [2] OK       "Story: tick labels"               (parent: 41)
#   batch add --dry-run: 2 ok, 1 review, 0 error — nothing written
```

- Runs the exact same parse/validate path as the real `run_add_core` (batch.rs:143-328) with the DB writes stubbed — **not** a reimplementation, or it will drift.
- Also emit the resolved defaults (priority/kind) so authors see what they'll get.

## P4 — `--fields` everywhere: `oneline` support + selectable aligned table

Current reality (verified): `--fields` works on `json` + `compact`; on `oneline` it prints `REVIEW: --fields is not supported…` and emits unfiltered output; `pretty` is aligned but fixed-column. This leaves the two formats agents/scripts most want field control over as the two that lack it.

```sh
# proposed
itr list --tag sprint-9 -f oneline --fields id,status,title     # TSV, chosen columns, script-ready
itr list -f pretty --fields id,status,blocked_by,title          # aligned table, chosen columns
```

- `oneline`: emit selected fields tab-separated in the requested order; list-valued fields (tags, blocked_by) join with `,`.
- `pretty`: build the column set from `--fields` instead of the fixed one; keep current default columns when the flag is absent.
- This closes the last reason to pipe itr into `python3 -c` at all.

## P5 — doc/help truth-sync (cheap, do with any of the above)

- README:408 still claims batch add is all-or-nothing ("if any issue fails validation, none are created") — stale since #164; rewrite to describe per-item soft-fallback.
- `--fields` help text says "JSON output" while CHANGELOG:197 claims "all formats"; after P4, both should say: all formats.
- README languages/feature table: document `bulk` verbs next to `batch` with a "which one do I want" line (filter-based vs explicit-list).
- `itr agent-info` (the machine-readable agent guide) must advertise P1–P4 syntax — the mined loops persisted mainly because agents didn't *know* `bulk` had landed. Discoverability is half this spec's value.

## Acceptance

1. Each mined loop shape in this spec's examples is expressible as one command, verified against a scratch DB (`ITR_DB_PATH=/tmp/itr-spec-test.db`).
2. `itr bulk depend --tag x --on N --dry-run` prints planned edges and writes nothing (DB byte-identical before/after).
3. `itr batch add --dry-run < payload.json` output matches the real run's REVIEW/ok verdicts for the same payload (golden test with one malformed item, one unknown key, one `@N` ref).
4. `--fields` on all four formats honors order and unknown-name soft-fallback (#49-style regression covered).
5. All mutations from multi-ID and bulk forms appear in `itr log` (audit), per the #35 regression.
6. README + agent-info updated in the same change set.

## Non-goals

- No new output formats, no TUI, no server mode.
- No change to single-ID verb semantics or exit codes.
- No speculative `bulk claim` — claiming stays deliberate.
