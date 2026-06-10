# Troubleshooting

This guide is symptom-based. Commands should be run from the project directory
unless a `--db` path is shown.

## `No .itr.db found`

Cause: the command could not find a database by walking up from the current
directory, and no override was supplied.

Check where you are:

```bash
pwd
find .. -name .itr.db -print
```

Initialize a tracker in the current project:

```bash
itr init
```

Use an explicit database:

```bash
itr --db /path/to/.itr.db stats
```

Or set an environment override:

```bash
export ITR_DB_PATH=/path/to/.itr.db
itr stats
```

`ITR_DB_PATH` takes precedence over `--db` and walk-up discovery (see
[environment.md](environment.md#itr_db_path) for the full precedence rules,
including the `itr init` inversion). Use it carefully in scripts so you do not
write to the wrong project database.

## `itr` Is Not Found

Check PATH:

```bash
command -v itr
echo "$PATH"
```

macOS and Linux installer defaults are `~/.local/bin` or `~/.cargo/bin` when it
is already on PATH. Add the install directory to your shell startup file if
needed:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Windows installs to `%LOCALAPPDATA%\Programs\itr` by default and adds that
directory to the user PATH. Restart the shell after install if `itr.exe` is not
found.

Verify the binary:

```bash
itr --version
```

## Install Download Or Checksum Fails

The install scripts download release archives from GitHub Releases and verify
SHA256 files when available.

Try:

```bash
ITR_VERSION=v0.1.0 ./install.sh
ITR_INSTALL_DIR="$HOME/.local/bin" ./install.sh
```

For source install fallback from a cloned repo:

```bash
ITR_FROM_SOURCE=1 ./install.sh
```

`ITR_VERSION` pins a release tag, `ITR_INSTALL_DIR` overrides the install
destination, and `ITR_FROM_SOURCE=1` forces a `cargo` build — see
[environment.md](environment.md#install) for full installer variable behavior.

On Windows:

```powershell
.\install.ps1 -Version v0.1.0
.\install.ps1 -InstallDir C:\tools\itr
```

If checksum verification fails, do not use the downloaded archive. Retry the
download or build from source.

## `itr ui` Cannot Bind Or Connect

`itr ui` binds to `127.0.0.1`. In sandboxes, local bind/connect may require
explicit permission.

Use an auto-selected port:

```bash
itr ui --no-open
```

Use a specific port:

```bash
itr ui --port 8787 --no-open
```

If the port is busy, choose another port or pass `--port 0` for automatic
selection. If browser opening fails, the server can still be used by copying the
printed URL into a browser on the same machine.

The URL contains a session token. Do not share it in logs or screenshots.

## UI API Returns `INVALID_VALUE` For Token

The UI API requires the current session token. Browser requests send
`X-ITR-Token`; direct calls must do the same or include the `token` query value.

Start UI and capture the URL:

```bash
itr ui --no-open -f json
```

Then call with the token header:

```bash
curl -H "X-ITR-Token: <token>" http://127.0.0.1:<port>/api/health
```

Restarting `itr ui` creates a new token.

## Search Misses Expected Results

`itr` tries FTS5 first when the virtual table exists and falls back to substring
search when FTS is unavailable or empty. If search looks stale, rebuild the FTS
index:

```bash
itr reindex
itr search "term" -f json
```

If `itr reindex` reports FTS5 unavailable, the SQLite build does not support
FTS5 in that environment. Normal search still falls back to LIKE-based matching.

See [docs/search.md](search.md) for full query semantics, the FTS5/LIKE
dispatch logic, and the complete list of indexed fields.

## `itr upgrade` Fails

`itr upgrade` finds source, optionally runs `git pull`, builds release, and
copies the new binary over the current executable.

Skip network pull and build current source:

```bash
itr upgrade --no-pull
```

Point at a source checkout:

```bash
itr upgrade --source-dir /path/to/itr --no-pull
```

Use `ITR_SOURCE_DIR` for scripts — see
[environment.md](environment.md#itr_source_dir) for the full source-directory
resolution order:

```bash
ITR_SOURCE_DIR=/path/to/itr itr upgrade --no-pull
```

Common causes:

- Source directory does not contain this repo's `Cargo.toml`.
- Current executable location is not writable.
- Cargo build fails.
- Git pull fails because the checkout has local conflicts or no network access.

When install location is not writable, use the installer or `cargo install`
instead of `itr upgrade`.

## JSON Parsing Fails In Scripts

Use `-f json` for machine parsing:

```bash
itr ready -f json
itr get 1 -f json
```

stdout is data. stderr is errors, warnings, hints, and `REVIEW:` notes. Do not
merge stderr into stdout unless you are intentionally collecting diagnostics.

Shell example:

```bash
OUT=$(itr ready -f json)
python3 -c 'import json,sys; print(len(json.load(sys.stdin)))' <<< "$OUT"
```

## Empty Results Are Not Errors

Empty list-like results exit 0. JSON mode prints `[]`.

```bash
itr list -f json
itr ready -f json
itr search "no-match" -f json
```

Use the exit code for hard failures such as missing DB, parse errors, not found,
or dependency cycles.

## Bulk Command Refuses To Run

Filter-based bulk operations require at least one filter so a broad operation is
not applied accidentally.

Use a filter:

```bash
itr bulk close --tag stale --dry-run
itr bulk update --status open --set-priority low --dry-run
```

Use batch operations when each item needs its own ID or payload:

```bash
itr batch close --dry-run < close-items.json
itr batch update --dry-run < update-items.json
```

## Doctor Reports Problems

Run:

```bash
itr doctor
```

For safe automatic cleanup:

```bash
itr doctor --fix
```

`doctor --fix` can remove orphaned dependencies, stale blocker relationships,
and rebuild stale FTS when available. It does not automatically resolve cycles,
stale in-progress issues, or empty epics.

## Updating An Existing Install

`install.sh` accepts `--update` to refresh an existing install in place rather
than dropping a new copy somewhere else on disk. The same script handles both
fresh installs and updates — `--update` only changes the log line and is most
useful in automation that wants to make the intent explicit.

```bash
./install.sh --update
curl -fsSL https://raw.githubusercontent.com/joeaguilar/itr/main/install.sh | bash -s -- --update
```

The prebuilt-binary update workflow:

1. Detect the host target (e.g. `aarch64-apple-darwin`,
   `x86_64-unknown-linux-musl`).
2. Resolve the release tag (pinnable via `ITR_VERSION` — see
   [environment.md](environment.md#itr_version)).
3. Download `itr-<tag>-<target>.tar.gz` and, when available, its `.sha256`
   companion. A checksum mismatch aborts the install.
4. Extract the archive into a temp directory.
5. Pick an install directory via `choose_install_dir` (see below) and copy
   the new binary over `itr` there, using `sudo install` when the
   destination is not writable.
6. Warn if the chosen directory is not on `PATH`.

If the prebuilt download fails (for example, no GitHub Releases asset for the
target, or no network), the script falls back to `cargo build --release`
provided the working directory is a cloned `itr` repo and `cargo` is
installed. To force the source path, set `ITR_FROM_SOURCE=1` (see
[environment.md](environment.md#itr_from_source)).

`install.sh --update` is the recommended way to move forward on a release
boundary. `itr upgrade` is the in-tree alternative — it expects a source
checkout, runs `cargo build --release`, and overwrites the current binary.
Use `itr upgrade` when you are working from source; use `install.sh --update`
when you installed from a release archive.

### `choose_install_dir` Precedence

`ITR_INSTALL_DIR` always wins when set; otherwise `install.sh` replaces an
existing `itr` on `PATH` in place (so updates do not leave a stale copy ahead
on `PATH`), then falls back to `~/.cargo/bin`, then `~/.local/bin`. The
Windows installer (`install.ps1`) defaults to `%LOCALAPPDATA%\Programs\itr`
and adds it to the user `PATH` if missing. See
[environment.md](environment.md#itr_install_dir) for the full fallback chain.

## WAL Companion Files (`.itr.db-wal`, `.itr.db-shm`)

`itr` runs SQLite in WAL (Write-Ahead Logging) mode. When the database has
been opened for writes, SQLite creates two companion files next to
`.itr.db`:

- `.itr.db-wal` — the write-ahead log holding pending changes.
- `.itr.db-shm` — the shared-memory index used to coordinate readers and
  writers.

When they appear:

- After any write (`add`, `update`, `close`, `claim`, `bulk`, `batch`, etc.).
- During an `itr ui` session, for as long as the server holds connections.
- They normally remain until the database is cleanly closed and a checkpoint
  runs; on a busy database they can stick around between command invocations.

Whether to commit or delete them:

- **Do not commit them.** Add `.itr.db-wal` and `.itr.db-shm` to
  `.gitignore`. They are local state, can contain data not yet merged into
  `.itr.db`, and will conflict in unhelpful ways across machines.
- **Do not delete them while any `itr` process is running.** Deleting them
  during an active session can lose pending writes or corrupt the database.
- **Safe to delete only when no `itr` process is running.** If the files
  remain after the last writer exits, opening the database with any `itr`
  command (for example `itr stats`) will checkpoint and tidy them up. If you
  must remove them manually (e.g. to ship a clean snapshot), make sure
  `itr ui` is stopped and no other `itr` invocation is in flight, then
  remove both `.itr.db-wal` and `.itr.db-shm`. The next open will recreate
  them as needed.
- **For backups, commit only `.itr.db`.** Run a clean `itr` command first to
  flush the WAL into the main database file, then copy `.itr.db` on its own.

## Error Code Reference

`itr` exits non-zero (always `1`) on hard failure and prints an error to
stderr. In `-f json` mode the message is wrapped as
`{"error": "...", "code": "..."}`. The full list of codes:

| Code             | When it fires                                                                 | Typical fix                                                                 |
|------------------|--------------------------------------------------------------------------------|-----------------------------------------------------------------------------|
| `NOT_FOUND`      | An issue ID does not exist.                                                    | Check the ID with `itr list` or `itr search`.                               |
| `CYCLE_DETECTED` | Adding a dependency would create a cycle.                                      | Drop one of the conflicting links with `itr undepend`, then retry.          |
| `INVALID_VALUE`  | A user-supplied field value did not normalize to a valid option.               | Use a listed value (see the error message for valid options).               |
| `NO_DATABASE`    | No `.itr.db` was found by walking up from the current directory.               | Run `itr init`, pass `--db`, or set `ITR_DB_PATH`. See top of this guide.   |
| `DB_ERROR`       | SQLite returned an error (lock contention, corruption, schema mismatch, etc.). | Retry; if persistent, run `itr doctor` and check for stale WAL companions.  |
| `PARSE_ERROR`    | JSON input to `batch` commands or stdin payloads was malformed.                | Validate the input with `python3 -m json.tool` and retry.                   |
| `IO_ERROR`       | Filesystem error reading or writing a file (permissions, missing path).        | Check the path and permissions reported in the error.                       |
| `UPGRADE_FAILED` | `itr upgrade` could not build, locate source, or overwrite the binary.        | See [`itr upgrade` Fails](#itr-upgrade-fails) above.                        |
| `NO_FILTERS`     | A `bulk` command was invoked with no filter (would touch every issue).         | Add at least one filter (`--status`, `--tag`, etc.) or use `batch`.         |

All errors exit `1`. Use the `code` field in JSON output to dispatch
recoverable conditions in scripts rather than parsing the human-readable
message.
