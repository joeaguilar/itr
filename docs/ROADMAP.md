# itr Roadmap

The cross-sprint planning map for `itr` — what's shipped, what's in flight,
what's left to clear the v1 bar, and what's parked for post-v1.

Audience: Product Owner + Scrum Master view, maintained by the `/roadmap`
skill at the start of `/sprint` and the close of `/sprint-review`.

This file is **not** the same as [`docs/limitations.md`](limitations.md). That
doc is user/contributor-facing and describes intentional constraints + known
bugs. This one is plan-facing and tracks the trajectory toward v1.

---

## Legend

### Status

- ✅ **Done** — shipped on `main`, verified.
- 🟡 **In progress** — actively being built; has open `itr` issues with code in flight.
- ❌ **Not started** — known v1 prereq; no code yet, may or may not have a tracked issue.

### Effort sizing

- **S** — Small. < 0.5 day. A focused PR / single-agent blitz task.
- **M** — Medium. 0.5–2 days. One sprint story, single owner.
- **L** — Large. 2–5 days. Spans a sprint, may need a small epic or coordinated wave.
- **XL** — Extra-large. > 5 days or a multi-sprint epic. Should be broken down before execution.

---

## v1 Feature-Complete Boundary

**v1 ships when the agent-first CLI is feature-complete for typical
issue-tracking workflows AND the output contract is stable enough that agent
prompts/skills can rely on it without breakage.**

Concretely, v1 requires:

1. **Core CLI surface complete** — add/list/get/update/close/note/depend/ready/next/claim/assign/wip/current/stats/summary/search/graph/log/relate/batch/bulk/export/import/reindex all present, documented, and tested via `tests/integration.sh`. ✅
2. **Soft-fallback philosophy applied consistently** — fuzzy normalization, REVIEW notes, no silent input drops. ✅
3. **Docs accuracy bar met** — every shipped doc verified against source; ≥10 docs ACCURATE, zero INCOMPLETE (tracked under epic #95).
4. **Output contract test suite** — snapshot harness covers every top-level command, alias, and format so future changes can't silently drift the agent-facing contract (tracked under epic #138).
5. **Installer parity** — `install.sh` and `install.ps1` both support `--update` and PATH-aware in-place upgrades.
6. **Local UI v1 workflows** — search/filter, add/edit, close/wontfix, notes, dependencies, relations, previewed bulk resolve, optional raw SQL behind `--allow-dangerous`. ✅
7. **Agent onboarding stable** — `itr agent-info`, `itr schema`, and `itr skill install` produce the canonical material an AI agent needs to drive the tool.

Anything not on that list is **post-v1**.

---

## Shipped (✅)

The CLI surface and supporting infrastructure that already meet the v1 bar.

| Item | Size | Notes |
| --- | --- | --- |
| Core issue CRUD (`add`/`get`/`list`/`update`/`close`) | L | Shipped; covered by integration tests. |
| Notes (`note`/`note-update`/`note-delete`) | M | Commit `ec3ec56`. |
| Dependencies + cycle detection (`depend`/`undepend`/`deps`/`ready`) | M | BFS cycle check in `db.rs`. |
| Workflow commands (`next`/`claim`/`start`/`wip`/`current`/`assign`/`unassign`) | M | `claim` accepts optional ID (`51a6178`). |
| Batch ops (`batch add`/`create`/`close`/`update`/`note` with `--dry-run`) | M | Unified `BatchResult` envelope (`4827ae3`). |
| Bulk ops (`bulk close`/`bulk update` with `--dry-run`) | S | Distinct from batch — clarified in `82a2466`. |
| Export / import (JSONL + JSON, `--merge`) | M | Importer event/relation drop fix in code-review blitz. |
| Search + `--tag-any` OR filter | M | Commits `6099016`, `9ece0a2`. |
| Urgency scoring engine with configurable coefficients | M | `urgency.rs`; docs in `docs/urgency.md`. |
| Output formats: compact / json / pretty / oneline | M | Oneline added in `089e87a`. |
| `--fields` soft-fallback filtering | S | `841ea98`. |
| Soft-fallback philosophy + REVIEW notes | M | `dadcfbc`; reference impl in `add.rs`/`update.rs`. |
| Audit `log` command with `--agent` filter | S | `5160758`. |
| Local browser UI (`itr ui`) with token auth, edits, notes, deps, relations, bulk preview | L | Commits `3586ded`, `c9b88bd`. |
| Raw SQL editor behind `--allow-dangerous` | S | `c9b88bd`. |
| Agent onboarding: `agent-info` / `schema` / `skill install` | M | `agent_docs.rs`, `commands/skill.rs`. |
| Installer with `--update` + PATH-aware upgrades (Unix) | M | `b229c87`. |
| Cross-platform release workflow + prebuilt installers | L | `b062683`. |
| Clippy pedantic + Rust quality pass | M | `2d3e453`, `aae26de`, plus code-review blitz `b68d42e`. |
| Docs sweep: 9 new docs + cross-doc verification | L | `b3c2814`; remaining polish under epic #95. |

---

## In Progress (🟡)

Actively being built or being cleared this week.

| Item | Size | Tracking | Notes |
| --- | --- | --- | --- |
| Docs v1 polish — post-verification sweep | L | epic #95 | All child issues closed in the 2026-05-19 blitz; epic closes on blitz completion. |
| Blitz orchestration: 22-task backlog clearance (5 waves) | L | epic #137 | Wave 5 in flight; closes on this wave landing. Follow-ups will be filed. |
| Bootstrap `docs/ROADMAP.md` for the `/roadmap` skill | S | #94 | This file. |
| Fix `itr-plan.md` drift left after historical-doc banner | S | #126 | Wave 5 sibling. |

---

## Not Started (❌)

Known v1 prereqs with no code yet. Sized so the next `/sprint` can pull them in.

| Item | Size | Tracking | Notes |
| --- | --- | --- | --- |
| Output contract test suite for CLI standard | XL | epic #138 | Snapshot harness covering every command, alias, format. Breaks into #140 (harness), #141 (help/no-db), #142 (core workflow), #143 (batch/bulk/import/export), #144 (UI smoke), #145 (origin-baseline diff tool). |
| Stabilize JSON output for `stats` and `graph` before snapshotting | M | #139 | Blocker for the snapshot suite — float precision + nested count maps need a deterministic contract. |
| Normalized CLI output snapshot harness | M | #140 | First brick of #138. |
| Snapshot core issue workflow outputs | L | #142 | Largest single snapshot task — ~27 commands × formats. |
| Snapshot help + no-database command outputs | M | #141 | |
| Snapshot batch / bulk / import / export outputs | M | #143 | |
| UI output + localhost API smoke contract tests | M | #144 | Needs sandbox guard for localhost permission. |
| Repeatable origin-baseline output diff tool | M | #145 | Distinguishes accepted standard changes from regressions. |

---

## Post-v1 Ideas

Parked. Not required to ship v1 — revisit after the contract test suite lands.

| Item | Size | Tracking | Notes |
| --- | --- | --- | --- |
| Multi-issue retrieval in a single command | L | #136 | Deferred from the 2026-05-19 blitz. Discovery + cross-surface audit (CLI, UI API, agent-info, SKILL.md, JSON schema, tests) before implementation. |
| Distributed sync / multi-user backend | XL | — | Explicitly out of scope per [`docs/limitations.md`](limitations.md). |
| Auth / permissions beyond UI token | L | — | Out of scope for local-first design. |
| MCP server surface | L | — | Mentioned in `itr-plan.md`; no commitment for v1. |
| Hard-delete in the local UI | S | — | Intentionally absent; pruning is via resolve/wontfix. |

---

## Maintenance Notes

- Update Shipped / In Progress / Not Started at the end of every `/sprint-review`.
- Re-anchor the v1 boundary if the Product Owner changes the bar — record the change in a short paragraph above, don't silently rewrite.
- Items here should map to either an `itr` issue (preferred) or a short rationale for why no issue exists yet.
- Keep this file consistent with [`docs/limitations.md`](limitations.md) (audience: users) and [`itr-plan.md`](../itr-plan.md) (audience: historical design). When they disagree, ROADMAP.md is authoritative for *trajectory*; limitations.md is authoritative for *current behavior*.
