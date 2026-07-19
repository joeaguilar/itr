# spec: control-plane DB addressing — directory `--db` + flag-over-env precedence

**Status:** implemented — shipped in `5f0837d` (2026-07-05, "feat(db)!: accept a directory for `--db` and let `--db` win over `ITR_DB_PATH`"). Retained as a design record; see `docs/environment.md` for current precedence rules.
**Originally proposed:** validated against installed `itr v2.13.1-dirty`, 2026-07-05; backlog scanned — 213 issues, 0 open, none covering this
**Origin:** Wisphive (a multiplexed AI-agent control plane) wants to create/update/close issues in *many* projects' trackers from one long-lived process, without `chdir`-ing per call. Filed downstream as Wisphive itr#476, which blocks Wisphive itr#474 ("Create itr issues from inside Wisphive") and itr#475 ("'Create itr issue' button on agent-history items"). This spec is the **upstream `itr` change** those depend on.

## What already works (no work — listed so this spec isn't re-litigated)

`itr` already has most of the addressing primitive. A control plane does **not** need a new flag or a server mode:

- **Global `--db <path>` flag** on every subcommand (`src/cli.rs:14`, `global = true`), resolved once in `src/main.rs:73` via `db::find_db(cli.db.as_deref())`.
- **`ITR_DB_PATH` env var** override (`src/db.rs:95`), documented in `docs/environment.md`.
- **No-junk-file guard (#160):** a nonexistent override path is rejected with a named-path error instead of `Connection::open` silently creating an empty broken DB the walk-up finder would then discover forever (`src/db.rs:117–142`).
- **Empty-string override is treated as unset** and falls through to walk-up (`src/db.rs:127`, `131`).

What follows are the two **residual** gaps that still block a per-project control plane today, plus doc sync.

---

## P1 — `--db` / `ITR_DB_PATH` must accept a **directory**, not only a `.itr.db` file

A control plane already knows each project's **root directory** (Wisphive tracks `cwd` per agent). It does not want to hand-construct the `.itr.db` suffix, and it can't `chdir` into the project to let walk-up do it. Today an override path is used **verbatim** as the SQLite file:

```rust
// src/db.rs:135 — resolve_override_db
if Path::new(path).exists() {
    Some(Ok(PathBuf::from(path)))   // used directly as the db FILE
}
```

So `--db /work/projectA` (a directory) either fails to open or, worse, is treated as a DB file. The caller is forced to know and append `/.itr.db`.

```sh
# today — caller must construct the file path itself
itr --db /work/projectA/.itr.db add "…"

# proposed — point at the project root; itr finds .itr.db inside
itr --db /work/projectA add "…"
```

**Semantics:**
- If the override path is a **directory**, resolve to `<dir>/.itr.db`.
- If it is a **file** (or a non-`.itr.db` filename the caller chose), use it verbatim — unchanged from today.
- Applies identically to `--db` and `ITR_DB_PATH` (shared `resolve_override_db`, and `src/commands/init.rs` which resolves separately — keep the two in sync).
- Error precision: a directory that exists but has **no `.itr.db`** must fail with a distinct message (`no .itr.db in <dir>; run 'itr init --db <dir>'`), not the current "path does not exist". Preserve the #160 guarantee — never create a junk file from a bad override.
- `itr init --db <dir>` creates `<dir>/.itr.db` (today `init` would create a file literally named after the directory).

## P2 — an explicit `--db` flag must win over an ambient `ITR_DB_PATH` (all commands, not just `init`)

This is the actual blocker. The precedence is currently **split**, and the non-`init` half is backwards for a control plane:

```
every command EXCEPT init (src/db.rs:126–128):   ITR_DB_PATH > --db > walk-up
itr init only        (src/commands/init.rs:10–17): --db > ITR_DB_PATH > cwd
```

`docs/environment.md:49–52` already articulates the correct rationale — *"an explicit flag has to be able to override an ambient `ITR_DB_PATH` that you set for a different project"* — but applies it **only to `init`**. A control plane exercises exactly that pattern on **every** verb: Wisphive may run with `ITR_DB_PATH` pointing at its **own** tracker, then pass `--db <projectX>` per call to add/close on a project's behalf. Under today's rules the ambient env **silently wins** and the write lands in the wrong database — a per-call correctness bug, not a UX wrinkle.

```sh
# today: env silently wins → writes to /control-plane/.itr.db, NOT projectA (data goes to the wrong tracker)
ITR_DB_PATH=/control-plane/.itr.db  itr --db /work/projectA close 42 "done"

# proposed: explicit --db wins everywhere, matching init and the documented rationale
ITR_DB_PATH=/control-plane/.itr.db  itr --db /work/projectA close 42 "done"   # → projectA
```

**Semantics:**
- Unify precedence across **all** commands to: **`--db` flag > `ITR_DB_PATH` env > walk-up from cwd**. `init`'s current order already matches; change is confined to `find_db`/`resolve_override_db` (`src/db.rs:93–143`).
- Empty `--db` is still "unset" (falls through to env, then walk-up) — unchanged.
- This is a **behavior change** for anyone who set `ITR_DB_PATH` *and* passed `--db` expecting env to win. That combination is nonsensical-by-design (why pass a flag you expect to be ignored?), it is confined to non-`init` commands, and it aligns them with `init` — call it out in CHANGELOG under a "behavior change" heading.

## P3 — doc/help truth-sync (cheap, do in the same change set)

- `docs/environment.md:37–52` — collapse the two precedence tables into the single unified order from P2; delete the "precedence asymmetry — `itr init` inverts this" note (no longer asymmetric).
- `docs/environment.md:33–35` — `ITR_DB_PATH` description says "Absolute path to the `.itr.db` SQLite file"; broaden to "path to a `.itr.db` file **or a directory containing one**" (P1).
- `docs/command-contracts.md` — global-flag/`--db` section: document directory acceptance and the unified precedence.
- `itr agent-info` (`src/agent_docs.rs`) — the machine-readable agent guide must advertise that `--db` accepts a directory and that the flag overrides `ITR_DB_PATH`; a control plane / agent reads this to know it can address any project by root path. Discoverability is half the value.

## Acceptance

1. `itr --db <dir> ready` opens `<dir>/.itr.db`; `itr --db <dir>/.itr.db ready` still opens the file verbatim (both verified against a scratch tree).
2. `itr --db <emptydir> ready` (directory exists, no `.itr.db`) exits non-zero with a message naming the directory and suggesting `itr init --db <emptydir>`; **no file is created** (dir byte-identical before/after — #160 preserved).
3. `itr init --db <dir>` creates `<dir>/.itr.db` (not a file named `<dir>`); re-running is idempotent.
4. With `ITR_DB_PATH=/A/.itr.db` set, `itr --db /B close N "x"` mutates `/B`, not `/A` — asserted on both databases. Same assertion for `add`, `update`, `note`, `get`.
5. With `ITR_DB_PATH` set and **no** `--db`, the env still wins over walk-up (regression: env override unbroken).
6. `docs/environment.md`, `docs/command-contracts.md`, and `itr agent-info` reflect the unified precedence and directory support in the same change set.

## Non-goals

- **No server/daemon mode, no long-lived connection pool.** A control plane invokes the `itr` binary per call; that is sufficient and keeps SQLite WAL + `busy_timeout` (`src/db.rs:150`) as the only concurrency story.
- **No new env var.** `ITR_DB` is *not* added as an alias — the canonical name stays `ITR_DB_PATH`; downstream (Wisphive) adopts the real name. (If an alias is later wanted, it is a separate, additive spec.)
- **No change to walk-up discovery**, the `.itr.db` filename, or exit codes beyond the P2 precedence flip.
- **No per-verb `--db` behavior differences** — resolution stays centralized in `find_db`.
