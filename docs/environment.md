# Environment Variables

This is the canonical reference for every `ITR_*` environment variable read by
`itr` — the CLI, the local UI server, the installer scripts, and the build /
upgrade machinery. Every variable below is optional; each one has either a
sensible default or an equivalent CLI flag.

Other docs that name an `ITR_*` variable should link here rather than
redescribe behavior.

## At a glance

| Variable | Scope | Read by | Purpose |
|---|---|---|---|
| `ITR_DB_PATH` | CLI runtime | `src/db.rs`, `src/commands/init.rs` | Override the `.itr.db` location. |
| `ITR_AGENT` | CLI runtime | `src/db.rs`, `src/commands/{next,note,batch}.rs` | Default agent identity for claims, notes, and audit events. |
| `ITR_SOURCE_DIR` | CLI runtime (upgrade) | `src/commands/upgrade.rs` | Override the source tree that `itr upgrade` rebuilds from. |
| `ITR_VERSION` | Install | `install.sh`, `install.ps1`, `build.rs` (set, not read) | Pin a specific release tag to install. |
| `ITR_INSTALL_DIR` | Install | `install.sh`, `install.ps1` | Override the install directory. |
| `ITR_FROM_SOURCE` | Install | `install.sh` | Force the installer to build from a cloned source tree. |
| `ITR_REPO` | Install | `install.sh`, `install.ps1` | Override the GitHub repo slug used for release downloads. |

Scopes:

- **CLI runtime** — read every time `itr` runs.
- **CLI runtime (upgrade)** — read only by `itr upgrade`.
- **Install** — read only by `install.sh` / `install.ps1` while installing or
  updating the binary; the running `itr` never reads them.

## CLI runtime

### `ITR_DB_PATH`

Absolute path to the `.itr.db` SQLite file. Skips the walk-up search that
normally locates the database from the current directory.

**Precedence (every command except `itr init`)** — `ITR_DB_PATH` wins:

1. `ITR_DB_PATH` (if set)
2. `--db <PATH>` flag
3. Walk-up search from the current directory for `.itr.db`

**Precedence asymmetry — `itr init` inverts this** — `--db` wins:

1. `--db <PATH>` flag (if supplied)
2. `ITR_DB_PATH` (if set)
3. `./.itr.db` in the current directory

The inversion is deliberate. `init` is how you create a database, so the
explicit flag has to be able to override an ambient `ITR_DB_PATH` that you set
for a different project. Every other command treats `ITR_DB_PATH` as an
intentional override that should not be silently shadowed by a stray flag.

If unset, `itr` walks up from the current directory looking for `.itr.db`,
which is how `cd`'ing into a project subdirectory still finds the project
database without any configuration.

Source: [`src/db.rs::find_db`](../src/db.rs),
[`src/commands/init.rs`](../src/commands/init.rs).

### `ITR_AGENT`

Default agent identity used when no `--agent` flag is supplied. Recorded on
claims, notes, and audit-log entries so multi-agent sessions stay attributable.

Read by:

- `itr next --claim` and `itr claim` / `itr start` — `--agent` flag wins, falls
  back to `ITR_AGENT`, otherwise no assignee is recorded.
- `itr note` — `--agent` flag wins, falls back to `ITR_AGENT`, otherwise the
  note's agent field is empty.
- `itr batch note` — per-item `agent` field wins, falls back to `ITR_AGENT`,
  otherwise empty.
- Event log writes — every audit event records `ITR_AGENT` (or empty) as the
  acting agent.

If unset, claims and notes still succeed; the agent field is just empty. There
is no authentication — `ITR_AGENT` is attribution only (see
[limitations.md](limitations.md#no-auth-system)).

Source: [`src/db.rs`](../src/db.rs),
[`src/commands/next.rs`](../src/commands/next.rs),
[`src/commands/note.rs`](../src/commands/note.rs),
[`src/commands/batch.rs`](../src/commands/batch.rs).

### `ITR_SOURCE_DIR`

Used only by `itr upgrade`. Points at the directory containing the `itr`
source tree (a checkout of this repo) that the upgrade should rebuild from.

**Precedence** — `itr upgrade` resolves the source directory in this order:

1. `--source-dir <PATH>` flag (hard-fails if it does not contain `itr`'s
   `Cargo.toml`)
2. `ITR_SOURCE_DIR` (silently skipped if it does not contain `itr`'s
   `Cargo.toml`)
3. The compile-time `CARGO_MANIFEST_DIR` baked into the running binary
4. Walk up from the running binary's path (up to 5 levels)
5. Walk up from the current directory

If none of the above point at a valid `itr` source tree, the command exits
with `UPGRADE_FAILED` and suggests setting `--source-dir` or `ITR_SOURCE_DIR`.

Source: [`src/commands/upgrade.rs::find_source_dir`](../src/commands/upgrade.rs).

## Install

These variables are read by the installer scripts only. The running `itr`
binary never looks at them. They are documented in the script headers
(`install.sh --help`, `install.ps1 -Help`) and reproduced here for a single
cross-platform reference.

### `ITR_VERSION`

Pin a specific release tag to install (e.g. `v0.1.0`). When unset, the
installer resolves the latest GitHub Release tag.

The same name is also used at **build time** by `build.rs`, which *sets*
`ITR_VERSION` for the `cargo:rustc-env` so `env!("ITR_VERSION")` resolves
inside `src/cli.rs` / `src/commands/upgrade.rs` / `src/commands/ui.rs`. The
build-time setter is independent of the installer-time reader — both happen to
share the name.

Read by: [`install.sh`](../install.sh), [`install.ps1`](../install.ps1).
Set by: [`build.rs`](../build.rs).

### `ITR_INSTALL_DIR`

Override the install directory. When unset, the installers walk a fallback
chain:

- **`install.sh`** — an existing `itr` on `PATH` (replace it in place), else
  `~/.cargo/bin` if it is on `PATH`, else `~/.local/bin`.
- **`install.ps1`** — an existing `itr.exe` on `PATH` (replace it in place),
  else `%LOCALAPPDATA%\Programs\itr`.

`install.sh` expands a leading `~` to `$HOME`. Both installers expand
environment-variable placeholders.

Read by: [`install.sh`](../install.sh), [`install.ps1`](../install.ps1).

### `ITR_FROM_SOURCE`

Set to `1` to skip the prebuilt-binary download path and build with `cargo`
instead. The installer must be run from inside a cloned `itr` source tree for
this to succeed: it runs `cargo build --release` and then installs
`target/release/itr` into the chosen install directory (using `sudo install`
when the directory isn't writable).

When unset (or anything other than `1`), the installer downloads the
appropriate prebuilt tarball for the detected target triple, verifies its
SHA256 checksum, and unpacks the binary into the install directory.

`install.ps1` does not implement `ITR_FROM_SOURCE` — Windows users who need
to build from source can run `cargo install --path .` directly.

Read by: [`install.sh`](../install.sh).

### `ITR_REPO`

Override the GitHub repo slug used for release downloads. Defaults to
`joeaguilar/itr`. Useful for testing release artifacts in a fork before
publishing.

Read by: [`install.sh`](../install.sh), [`install.ps1`](../install.ps1).

## Quick examples

```bash
# Use a specific database from a different project root
ITR_DB_PATH=/work/projectA/.itr.db itr ready

# Initialize a database at a deliberate path despite ITR_DB_PATH being set
ITR_DB_PATH=/work/projectA/.itr.db itr init --db /work/projectB/.itr.db

# Identify a long-running agent session for audit attribution
export ITR_AGENT=claude-session-001
itr claim
itr note 42 "Investigated; root cause is connection pool exhaustion."

# Install a pinned release into a specific directory
ITR_VERSION=v0.1.0 ITR_INSTALL_DIR="$HOME/.local/bin" ./install.sh

# Build the installer from your local clone instead of downloading
ITR_FROM_SOURCE=1 ./install.sh

# Upgrade from a specific source checkout
ITR_SOURCE_DIR=/work/itr itr upgrade --no-pull
```

## See also

- [Command contracts](command-contracts.md) — global flag and `--db`
  precedence for every command.
- [Architecture](architecture.md) — where database discovery happens in the
  CLI flow.
- [Troubleshooting](troubleshooting.md) — install, upgrade, and database
  recovery procedures that reference these variables in context.
- [Limitations](limitations.md#no-auth-system) — why `ITR_AGENT` is
  attribution, not authentication.
