# Rust Best Practices Score: `itr`

Scored against The Comprehensive Rust Best Practices Reference (informal internal guide; no
checked-in reference document — earlier link removed).
Codebase: ~9,940 LOC of `src/`, single binary, synchronous CLI, SQLite-backed, no network I/O.

---

## Overall Score: **A- (87/100)**

| Section | Score | Weight | Weighted |
|---------|-------|--------|----------|
| Ownership & Borrowing | 92/100 | 20% | 18.4 |
| Error Handling | 88/100 | 20% | 17.6 |
| API Design | 82/100 | 20% | 16.4 |
| Performance & Memory | 82/100 | 15% | 12.3 |
| Code Organization | 88/100 | 15% | 13.2 |
| Testing & Tooling | 86/100 | 10% | 8.6 |
| **Total** | | **100%** | **86.5** |

---

## 1. Ownership & Borrowing — 92/100

### What the guide says
> Start with owned data in structs, borrow in function parameters. Reach for smart pointers or interior mutability only when your design genuinely requires it.

### What we do

**GOOD (follows guide):**

- **Structs own their data.** All models (`Issue`, `IssueDetail`, `IssueSummary`, etc.) use `String`, `Vec<String>`, `i64` — appropriate for serde serialization and DB row mapping. (`models.rs`)
- **Functions borrow parameters.** Consistent `&str`, `&[String]`, `&Connection` across all command handlers and `db.rs`. Example: `insert_issue()` takes `&str` for all string fields. (`db.rs`)
- **`as_deref()` used correctly.** `main.rs` converts `Option<String>` to `Option<&str>` before passing to DB layer. (`main.rs:73`)
- **No unnecessary smart pointers.** `Box<dyn ToSql>` used only where rusqlite requires trait objects. No `Rc`, `Arc` (not needed — single-threaded, no shared ownership).
- **Interior mutability justified.** Single `RefCell` in a `thread_local!` for `--fields` filter state — correct pattern for single-threaded thread-local. (`format.rs:5-8`)
- **No explicit lifetimes.** None needed — Rust's elision rules handle all cases. Functions are short-lived borrows with no complex reference hierarchies.
- **Borrowed/owned variant pair for hot construction path.** `build_issue_summary()` (borrowing wrapper) plus `build_issue_summary_owned()` (consumes the `Issue`) — list/ready paths now consume by-value via `into_iter()` and move string/vec fields directly into the `IssueSummary` instead of cloning every field. (`commands/mod.rs:39-84`, `commands/list.rs:25-32`, `commands/ready.rs:44-50`)

**ACCEPTABLE (pragmatic trade-offs):**

- **60 `.clone()` calls total.** Down from the previous summary's accounting once the per-field clones in `build_issue_summary` were eliminated — the remaining clones are in cold paths (stats aggregation, batch processing, JSON wrapping) and a handful in UI handlers (`ui.rs` builds summaries from borrowed iterators). The wrapper itself only clones the whole `Issue` once when a caller cannot give up ownership (get/summary/ui).
- **`Cow<str>` not used.** The guide recommends it for data that "often passes through unmodified." Not applicable here — DB rows produce owned `String`s, and the CLI doesn't do passthrough processing.

**NOT APPLICABLE:**

- `mem::take`/`mem::replace` — no state machine transitions or move-out-of-`&mut` patterns.
- Entry API — used correctly in the one HashMap aggregation path (`stats.rs`).

### Practical impact of full adherence
The remaining clone pressure is in the borrowing-wrapper callsites (get/summary/ui), which iterate slices and reuse each `Issue` after summary construction. Switching those callers to consume-by-value would force a structural change in `ui.rs` (which interleaves summaries with detail builders). **Performance impact at current scale: negligible** — the dominant hot path (list/ready) is already owned-input.

---

## 2. Error Handling — 88/100

### What the guide says
> Use `thiserror` for modules where callers match on variants; `?` for propagation; `unwrap()` only for proven invariants; separate `main()` from `run()`.

### What we do

**GOOD:**

- **`thiserror` error enum.** `ItrError` uses `#[derive(thiserror::Error)]` with `#[from]` for `rusqlite::Error`, `serde_json::Error`, and `std::io::Error`. Each variant has a machine-readable error code. (`error.rs`)
- **`?` operator used consistently.** ~500 use sites across `src/` — all command handlers return `Result<(), ItrError>` and propagate cleanly.
- **`main()`/`run_command()` separation.** `main()` handles parsing and DB discovery; `run_command()` dispatches to handlers. Errors bubble up and are handled by `handle_error()` which prints to stderr (JSON in json mode) and exits. (`main.rs:39-89`, `main.rs:91`+)
- **No bare `unwrap()` on I/O.** Every `unwrap()` has a fallback: `unwrap_or_default()`, `unwrap_or_else()`, or `unwrap_or()`. Zero naked `unwrap()` calls on user input or I/O operations.
- **Soft fallbacks philosophy.** Invalid priority/kind/status values default to safe values with `REVIEW:` stderr notes instead of hard errors. Reference implementation in `add.rs` and `normalize.rs`.
- **DB query failures now surface, not silent.** `urgency.rs` previously used silent `unwrap_or(false)`; now uses `.unwrap_or_else(|e| { eprintln!("REVIEW: …"); … })` for `is_blocked`, `blocks_active_issues`, and `count_notes`. Failures are visible to operators without killing the run. (`urgency.rs:230,243,275`)

**ACCEPTABLE:**

- **`process::exit()` used in 2 places.** `handle_error()` and format validation in `main.rs:47`. Both are terminal — no open file handles or incomplete transactions. The guide prefers `ExitCode` return, but this pattern (BurntSushi-style) is explicitly acknowledged as widely used.
- **No `.context()` chaining.** The project uses `thiserror` (not `anyhow`), so `.context()` isn't available. Error variants carry their own context (`NotFound(i64)`, `InvalidValue { field, value, valid }`). Adequate for a small CLI.

**NEEDS IMPROVEMENT:**

- **FTS5 setup failures still silently ignored.** `let _ = conn.execute_batch(...)` discards errors when FTS5 is unavailable. Acceptable (FTS5 is optional) but a debug log would help diagnosis. (`db.rs` FTS5 init blocks)

### Practical impact of full adherence
The remaining gap (FTS5 silent failure) is a one-line change to emit a `REVIEW:` note when FTS5 isn't usable. Switching from `process::exit()` to `ExitCode` return would require refactoring `handle_error()` to return instead of exit — moderate effort, no performance impact, marginal safety improvement (destructors already run properly in current paths).

---

## 3. API Design — 82/100

### What the guide says
> Newtype pattern for validation. Builder pattern for >5 parameters. Accept broad, return specific. Derive standard traits eagerly. Make illegal states unrepresentable.

### What we do

**GOOD:**

- **RFC 430 naming.** `CamelCase` types, `snake_case` functions, `SCREAMING_SNAKE` constants. No violations found.
- **Standard traits derived.** All public structs have `#[derive(Debug, Clone, Serialize, Deserialize)]`. `Format` enum additionally derives `Copy, PartialEq`. (`models.rs`, `format.rs`)
- **Enum for closed sets.** `Format` is a proper enum with exhaustive matching. `Commands`, `BatchAction`, `BulkAction`, `ConfigAction` use clap's derive enums. (`cli.rs`, `format.rs`)
- **Trait design is minimal and focused.** One custom trait: `HasUrgency` with a single method, used for generic sorting. Two implementors. (`commands/mod.rs:95-110`)
- **`ListFilter` struct now in use.** `db::list_issues(conn, &ListFilter)` replaces the previous 12-parameter call. Default values via `Default` derive let callers construct partial filters with `..ListFilter::default()`. (`models.rs:3-17`, `db.rs:347-350`, `commands/list.rs`, `commands/ready.rs`)

**ACCEPTABLE (intentional deviations):**

- **No newtype pattern for IDs, priority, status, kind.** These are raw `i64` and `String`. The guide says newtypes prevent type confusion. **Exception called out:** the soft-fallbacks philosophy explicitly requires accepting bad input and defaulting gracefully — newtypes with validating constructors would reject input at parse time, which conflicts with the design goal of never failing on agent input when a default exists. Runtime validation via `normalize.rs` + `REVIEW:` notes is the intentional alternative.
- **Strings for status/priority/kind instead of enums.** The guide prefers enums for closed sets. **Exception called out:** SQLite `CHECK` constraints enforce validity at the storage layer. The normalization layer maps synonyms (`urgent`→`critical`, `wip`→`in-progress`). Using String allows soft fallbacks — an enum would force a hard parse error. This is a conscious trade-off documented in CLAUDE.md.
- **No `impl Into<String>` or `impl AsRef<Path>`.** Functions accept concrete types. For a binary crate (not a library), this is appropriate — no external callers need generic ergonomics.

**NEEDS IMPROVEMENT:**

- **Remaining high-arity handlers.** `add::run` still takes 16 args (`commands/add.rs:33`); `update::run` is similar. Both keep `#[allow(clippy::too_many_arguments)]`. These mirror the clap-derived arg list one-to-one — an intermediate `AddOptions` struct would help but is lower-value now that the `ListFilter` precedent exists.
- **No `pub(crate)` annotations.** All module-internal functions use default visibility. For a small binary crate this works, but explicit visibility would document intent.
- **`PartialEq`/`Eq` not derived on models.** Not commonly needed in this codebase, but would enable easier testing assertions.

### Practical impact of full adherence
- **`AddOptions`/`UpdateOptions` structs:** Mirrors the `ListFilter` pattern, moderate refactor (~60 lines across `commands/add.rs` + `commands/update.rs` + their clap mappings in `main.rs`). Zero performance impact. Improves readability and lets future fields (e.g., `--epic`) ride without bumping every call.
- **Newtypes for priority/status/kind:** Large refactor (~200+ lines). Would **negatively impact** the soft-fallbacks philosophy — the whole point is accepting messy input. **Not recommended.**
- **`pub(crate)` annotations:** Trivial (~30 one-line changes). Zero performance impact. Documentation-only benefit.

---

## 4. Performance & Memory — 82/100

### What the guide says
> Use `with_capacity()` when size is known. Avoid unnecessary `format!()`. Configure release profile (LTO, codegen-units). Reuse collections.

### What we do

**GOOD:**

- **No async.** Synchronous code is correct for a local CLI with no network I/O. The guide explicitly says: "many CLI tools work perfectly with synchronous code."
- **Iterator chains used idiomatically.** `.filter().map().collect()` throughout. No manual loops where iterators would suffice. (`db.rs` list builder, `util.rs`)
- **Minimal dependency footprint.** 6 direct runtime dependencies, 1 dev dependency (`proptest`), no async runtime, no network crates. Compile times are fast (~3.5s release).
- **Release profile tuned.** `Cargo.toml` carries `[profile.release]` with `lto=true`, `codegen-units=1`, `strip=true` — the binary ships ~50% smaller than the cargo defaults. (`Cargo.toml:12-15`)
- **Densest clone site refactored.** `build_issue_summary` is split into a borrowing wrapper and an owning `build_issue_summary_owned`. The list/ready hot paths consume `Vec<Issue>` via `into_iter()` and move each issue's string/vec fields straight into the summary — the previous per-field 11-clone storm is gone for those callers. (`commands/mod.rs:46-84`)

**ACCEPTABLE:**

- **No `with_capacity()` anywhere.** Vectors are allocated with `Vec::new()` then grown dynamically. For typical issue counts (<1000), reallocation overhead is negligible. The guide says this matters for "known size" — most vectors here have unknown size (DB query results).
- **171 `format!()` calls, 226 `.to_string()` calls (whole `src/`).** Counts grew slightly with new code; many are unavoidable (building SQL placeholders, output strings). Some could be replaced with string literals or `write!()` to a buffer, but the guide acknowledges "profile before optimizing" and the `Cargo.toml` lint config explicitly tolerates `format_push_string` for readability.

**NEEDS IMPROVEMENT:**

- **`build_issue_summary` (borrowing wrapper) still clones the whole `Issue` once per call.** Used by `get`, `summary`, and `ui.rs` (3 sites). Lower cost than the previous per-field storm, but the next optimization would be to teach those callers to consume by value — `ui.rs` is the trickiest because it interleaves summaries with detail builders.
- **SQL placeholder generation allocates per-placeholder.** `format!("?{}", i)` in a loop still creates N small strings inside `append_in_clause`. Could use a single `write!()` to a shared buffer; impact bounded by the number of `IN (…)` parameters.

### Practical impact of full adherence
- **`with_capacity()`:** Sprinkle in ~10 call sites. Zero risk, marginal improvement. Worth doing when touching those paths.
- **Reducing `format!()`:** Diminishing returns. Most calls are in cold paths (error messages, output formatting).
- **Pushing the owned variant further:** Refactoring `ui.rs` callsites to consume by value would remove the last `Issue::clone()` calls in the summary pipeline; modest gain, modest code churn.

---

## 5. Code Organization — 88/100

### What the guide says
> Keep `main.rs` thin. Group by domain/feature. Use `pub(crate)` for internal helpers. Re-export key types.

### What we do

**GOOD:**

- **Thin `main.rs`.** ~470 lines, of which the top 89 are orchestration and the rest is the `run_command` dispatch match. No business logic. (`main.rs`)
- **Domain-organized modules.** `commands/` directory with one file per command (30 `.rs` files). `db.rs` for storage, `format.rs` for output, `urgency.rs` for scoring, `normalize.rs` for input normalization. Clean separation.
- **Single responsibility per command handler.** Each `commands/*.rs` exports a `run()` function. Cross-command logic is centralized in `commands/mod.rs` (`build_issue_summary`, `build_issue_summary_owned`, `build_issue_detail`, `sort_by_urgency_desc`, `print_detail_with_unblocked`).
- **`db.rs` is monolithic but intentional.** ~1,190 lines containing all SQLite operations. Documented in CLAUDE.md as the largest file. Splitting would create artificial boundaries since all operations share the schema and connection.

**ACCEPTABLE:**

- **No crate-root re-exports.** Binary crate — no external consumers. Internal `use crate::models::*` imports are clear enough.
- **`commands/mod.rs` has shared helpers** including both `build_issue_summary` variants. These are the right abstraction level — shared across list/ready/search/next/get/summary/ui.

### Practical impact of full adherence
Already well-organized. `db.rs` is now ~1,190 LOC (up from ~730 at the previous snapshot, reflecting new migrations / FTS5 / events). It is still cohesive — all operations share schema + connection. A 1,500-LOC threshold for splitting remains reasonable but is approaching.

---

## 6. Testing & Tooling — 86/100

### What the guide says
> Unit tests in `#[cfg(test)]` modules. Integration tests for public API. Doctests as documentation. Clippy with `pedantic`. CI pipeline: fmt, clippy, test, deny.

### What we do

**GOOD:**

- **Comprehensive integration test suite.** `tests/integration.sh` — ~2,070 lines with 305 assertions covering all commands, edge cases, soft fallbacks, aliases, batch operations, multi-agent workflows, FTS search, UI smoke, etc. Uses `python3 -c` for JSON parsing. (`tests/integration.sh`)
- **CI pipeline enforces quality.** GitHub Actions: `cargo fmt --check` → `cargo clippy -- -D warnings` → release build + integration tests. Auto-versioning on push to main. (`.github/workflows/ci.yml`)
- **`justfile` provides developer shortcuts.** `just test`, `just lint`, `just verify` (full pre-push validation). (`justfile`)
- **Clippy `pedantic` enabled.** `Cargo.toml` carries `[lints.clippy] all = warn, pedantic = warn` with a short, explicitly-documented allowlist for noisy lints that conflict with project conventions (soft fallbacks, format-style choices, SQLite numeric casts). `dbg_macro` is denied. (`Cargo.toml:28-54`)
- **Property-based tests added.** `proptest` is a dev-dependency; `normalize.rs` carries 16 `prop_*` cases (lowercase invariant, idempotence, canonical roundtrip, case-insensitivity, synonym→canonical→validation roundtrip for all three of priority/kind/status, plus an unknown-input passthrough) and `util.rs` carries the comma-parsing / tag normalization properties — covering the broad input spaces the shell tests can only sample. Originally framed as "27 property tests"; the on-disk count after wave 3 is 30 `prop_*` functions. (`src/normalize.rs` tests module, `src/util.rs` tests module, `Cargo.toml:25-26`)
- **Doctests added on key public functions.** `///` example blocks (ignore-marked because this is a binary crate, not a library) on the headline functions in `normalize.rs` (8 blocks), `util.rs` (7), `urgency.rs` (5), `format.rs` (10). They render in `cargo doc` and document expected inputs/outputs even though they don't execute in CI.

**ACCEPTABLE:**

- **Unit tests concentrated in a few modules.** `util.rs`, `normalize.rs`, `urgency.rs`, and `format.rs` carry `#[cfg(test)]` modules (62 `#[test]` items combined, including the new `proptest!` cases). `db.rs` and most command handlers still lean on the integration suite for coverage. Acceptable given the integration suite's breadth, but unit tests would catch regressions faster and localize failures.

**NEEDS IMPROVEMENT:**

- **No snapshot testing.** `insta` would still be ideal for CLI output validation — currently done via shell string matching which is brittle when output format evolves.
- **No `cargo-deny` or `cargo-audit`.** Dependency security scanning not configured. Low risk (6 deps + 1 dev-dep, all well-known) but good hygiene.

### Practical impact of full adherence
- **`cargo-deny`/`cargo-audit`:** 5 minutes to add to CI. No code changes.
- **`insta` snapshots:** Adds 1 dev-dependency. Higher effort (~4-8 hours) but would catch output regressions the shell tests miss. **Recommended for `format.rs` outputs.**
- **More unit tests in `db.rs`/handlers:** Steady incremental work — the next regression that survives integration tests is the natural trigger.

---

## Exceptions Register

Deviations from the guide that are **intentional and should be preserved**:

| Deviation | Guide Recommendation | Why We Differ | Reference |
|-----------|---------------------|---------------|-----------|
| No newtypes for priority/status/kind | Newtype pattern for validated data | Soft-fallbacks philosophy requires accepting bad input and defaulting gracefully | CLAUDE.md "Soft Fallbacks Philosophy" |
| Strings instead of enums for data values | Enums for closed sets | Must allow normalization of synonyms and graceful fallback to defaults | `normalize.rs`, CLAUDE.md |
| `process::exit()` in error handler | Return `ExitCode` from main | BurntSushi-style pattern; guide acknowledges as "widely used" | `error.rs` |
| No `anyhow` / `.context()` | `anyhow` at application boundary | `thiserror` enum is sufficient for small CLI; error variants carry their own context | `error.rs` |
| No async | Use async for concurrent I/O | No network I/O; guide says "many CLI tools work perfectly with synchronous code" | `Cargo.toml` |
| `db.rs` monolithic (~1,190 LOC) | Split by domain | All operations share schema/connection; splitting adds files without reducing complexity | CLAUDE.md |
| `#[allow(clippy::too_many_arguments)]` on `add::run` / `update::run` | Use structs for >5 params | Direct CLI arg mapping; the `ListFilter` precedent shows the pattern when we choose to apply it | `commands/add.rs:32`, `db.rs:347` |
| Doctests marked `ignore` | Doctests should execute | Binary crate — public functions are not callable as a library, so executable doctests would require a library facade | `normalize.rs`, `util.rs`, `urgency.rs`, `format.rs` |

---

## Recommended Improvements (by effort/impact)

### Quick Wins (< 30 min, zero risk)

1. **Add `cargo-deny`** to CI — dependency license/advisory scanning. 5-minute setup.
2. **Emit `REVIEW:` notes** when FTS5 setup fails in `db.rs` instead of `let _ = ...`.

### Medium Effort (1-4 hours, low risk)

3. **Add `with_capacity()`** to known-size vector allocations in `db.rs` and command handlers.
4. **Extract `AddOptions` / `UpdateOptions` structs** to mirror the `ListFilter` precedent and tame the remaining `#[allow(clippy::too_many_arguments)]` sites.
5. **Push the owned summary variant into `ui.rs`** — convert the three remaining `build_issue_summary(conn, issue, &config)` callsites to consume by value where possible.

### Higher Effort (4-8 hours, moderate risk)

6. **Snapshot tests for `format.rs`** — add `insta` and pin the compact/pretty/JSON output shapes; reduces brittleness of `tests/integration.sh` string assertions.
7. **More unit tests in `db.rs`** — currently leans entirely on integration tests; targeted unit tests would localize the next regression.

### Done Since Previous Snapshot

- ~~Add release profile (LTO, codegen-units=1, strip)~~ — landed in `Cargo.toml:12-15`.
- ~~Enable clippy `pedantic`~~ — landed in `Cargo.toml:28-54` with a curated allowlist.
- ~~Emit `REVIEW:` notes for masked DB errors in `urgency.rs`~~ — landed at `urgency.rs:230,243,275`.
- ~~Extract `ListFilter` struct~~ — landed in `models.rs:3-17`, consumed by `db::list_issues`, `commands/list.rs`, `commands/ready.rs`.
- ~~Add `proptest` tests for `normalize.rs` and `util.rs`~~ — landed in the 2026-05-19 blitz (closes itr #69).
- ~~Add doctests to key public functions in `normalize.rs`, `util.rs`, `urgency.rs`, `format.rs`~~ — landed in the 2026-05-19 blitz (closes itr #70).
- ~~Reduce clone concentration in `build_issue_summary()`~~ — split into borrowed wrapper + `build_issue_summary_owned`; list/ready paths now consume by value (closes itr #71).

### Not Recommended (high effort, negative or negligible impact)

- Newtype wrappers for priority/status/kind — conflicts with soft-fallbacks philosophy.
- Split `db.rs` into sub-modules — still under the ~1,500 LOC threshold; artificial boundaries hurt cohesion.
- Switch to `anyhow` — `thiserror` enum is sufficient and provides machine-readable error codes.
- Add `Cow<str>` to models — complicates serde for negligible performance gain at current scale.

---

## Scoring Methodology

Each section scored on:
- **Adherence** (0-40): How closely the code follows the guide's recommendations.
- **Justification** (0-30): How well deviations are justified by project constraints.
- **Consistency** (0-30): How uniformly patterns are applied across the codebase.

Weights reflect relative importance for a synchronous CLI tool (ownership/errors weighted higher than performance/async).

---

*Refreshed 2026-05-19 by Claude Opus 4.7 against the `itr` codebase. Repository HEAD at refresh time was `b68d42e`; this snapshot reflects state **after the 2026-05-19 blitz** (issues #69 proptest, #70 doctests, #71 clone refactor) which is staged on local `main` but not yet committed. Re-pin when the blitz commit lands.*
