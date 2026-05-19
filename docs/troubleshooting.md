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

`ITR_DB_PATH` takes precedence over walk-up discovery. Use it carefully in
scripts so you do not write to the wrong project database.

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

Use `ITR_SOURCE_DIR` for scripts:

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
