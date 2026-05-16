# Story Style — itr

_Last updated: 2026-05-15_

> How this project writes issues. Read by `/sprint` Phase 0 and any agent that creates issues for this repo.

## Title & Body

**Title shape:** imperative — use action-oriented titles like "Fix release workflow trigger" or "Add release smoke tests".
**Title length:** soft cap at 80 characters.
**Title prefix:** none — use tags, files, and skills for metadata instead.

**Body template:**

Context:
One concise paragraph explaining why this matters, the failure mode, and any relevant constraints.

Notes:
Optional implementation notes, references, edge cases, or prior investigation.

**Required sections:** Context
**Optional sections:** Notes

## Acceptance Criteria

**Format:** bulleted observable outcomes.
**Observability rule:** acceptance criteria must be agent-checkable through commands, files, behavior, output, or clearly inspectable state.
**DoD reference:** sprint-specific Definition of Done is appended by `/sprint`; it is not defined here.

## Tags & Priority

**Tag taxonomy:** flat tags, such as `ci`, `release`, `testing`, `docs`, `install`, `windows`, `linux`, or `rust-score`.
**Priority scheme:** `critical`, `high`, `medium`, `low`.
**Epic linking:** use `--parent <id>` only when there is a real parent issue.

## Language & Voice

**Terminology:** prefer "issue".
**Voice:** terse-technical, agent-oriented.

**Banned phrases / anti-patterns:**
- Avoid vague acceptance criteria like "works properly" or "handles things better".
- Avoid filler words like "simply" and "just".
- Avoid titles that only say "improve" without naming the observable target.

**Domain glossary:**
- **itr** — this repo's local, SQLite-backed issue tracker CLI.
- **agent-first** — optimized for AI coding agents: parseable output, deterministic behavior, low setup.
- **compact output** — token-efficient plain output intended for agent consumption.
- **soft fallback** — accepting near-valid input while marking it for review instead of hard failing.
- **ready issue** — an unblocked non-terminal issue suitable for immediate work.
- **release artifact** — a packaged binary archive and checksum uploaded to a GitHub Release.

**Other project-specific notes:**
- Include `--files` for implicated code, docs, workflows, or scripts whenever known.
- Include `--skills` when specialized capability helps route work, such as `devops`, `rust`, or `powershell`.

## Worked Examples

### Example 1 — task

Add release smoke tests before uploading artifacts

Context:
The release workflow builds and packages target binaries but does not run a minimal behavior check before uploading release assets. A broken binary or missing runtime behavior could be published.

Notes:
Use a temp directory for database operations so release jobs do not depend on repo state.

**Acceptance criteria:**
- Release jobs run `itr --version` before packaging.
- Release jobs run a temp-dir `init/add/ready` smoke flow before upload.
- Upload is blocked when a native smoke test fails.

### Example 2 — bug

Fix auto-version tags so release workflow runs

Context:
The auto-version workflow creates and pushes release tags with the default GitHub Actions token. Tag pushes created this way may not trigger the release workflow, leaving a version tag without uploaded assets.

Notes:
Keep manual release dispatch working while fixing the automatic path.

**Acceptance criteria:**
- A qualifying push to `main` creates a version tag and starts a release run.
- The release run uploads expected platform archives and checksum files.
- The solution avoids relying on tag-push events that GitHub suppresses for `GITHUB_TOKEN`.
