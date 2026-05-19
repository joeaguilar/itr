# Urgency Scoring

Every issue in `itr` has a computed **urgency score** that drives `itr next`,
`itr ready`, and the default sort order of `itr list`. The score is **never
stored** — it is always computed fresh from the current state of the issue and
its relations. This means tuning a coefficient with `itr config set` takes
effect on the very next read, with no recompute step required.

This document is the source-of-truth reference for the scoring formula. The
implementation lives in [`src/urgency.rs`](../src/urgency.rs).

## Formula

Urgency is a simple additive scalar. Each enabled component contributes a
signed number that is summed into a single total:

```
urgency = priority + kind + blocking + blocked + age + in_progress + has_acceptance + notes
```

Negative components (e.g. `blocked`, `kind.epic`) deliberately push an issue
down the ready queue. There is no clamp on the total — strongly blocked work
can score negative, and a critical, in-progress, blocking, bug-tagged issue
can score well above 30.

### Per-component math

| Component | When it applies | Contribution |
|-----------|-----------------|--------------|
| `priority.<bucket>` | Always | Coefficient looked up by the issue's `priority` value. Unknown buckets contribute `0`. |
| `kind.<bucket>` | Always | Coefficient looked up by the issue's `kind` value. Unknown buckets contribute `0`. |
| `blocking` | The issue blocks at least one other active (non-terminal) issue | `+ config.blocking` |
| `blocked` | The issue is currently blocked by another open dependency | `+ config.blocked` (default is negative) |
| `age` | Always | `config.age * clamp(days_since_created / 10, 0, 1)` — ramps linearly to full weight over 10 days, then plateaus |
| `in_progress` | `status == "in-progress"` | `+ config.in_progress` |
| `has_acceptance` | The issue's `acceptance` field is non-empty | `+ config.has_acceptance` |
| `notes` | The issue has at least one note | `config.notes_count * min(notes_count / 6, 1)` — caps at the full coefficient once the issue has 6 or more notes |

The breakdown returned by `compute_urgency_with_breakdown` lists every applied
component along with its numeric contribution, so `itr get <ID> -f json` shows
exactly how a score was assembled.

## Coefficient Table

These are the full set of keys read by `UrgencyConfig`. All of them can be
overridden via `itr config set <key> <value>`; unset keys fall back to the
defaults below. Unknown or unparseable values in the `config` table are
silently ignored — defaults stay in place. This is the standard soft-fallback
behavior for the urgency system.

| Config Key | Default | Notes |
|------------|---------|-------|
| `urgency.priority.critical` | `10.0` | Top priority bucket |
| `urgency.priority.high` | `6.0` | |
| `urgency.priority.medium` | `3.0` | |
| `urgency.priority.low` | `1.0` | |
| `urgency.blocking` | `8.0` | Added when this issue blocks others — surfaces work that unblocks the most downstream tasks |
| `urgency.blocked` | `-10.0` | Subtracted when this issue is blocked — pushes it down so `itr ready` skips it |
| `urgency.age` | `2.0` | Maximum age contribution. Scales linearly: `0` days → `0`, `10` days → full coefficient, plateaus after 10 days |
| `urgency.has_acceptance` | `1.0` | Rewards issues with testable acceptance criteria |
| `urgency.kind.bug` | `2.0` | Bugs get a small boost over other kinds |
| `urgency.kind.feature` | `0.0` | Neutral by default |
| `urgency.kind.task` | `0.0` | Neutral by default |
| `urgency.kind.epic` | `-2.0` | Epics are containers, not direct work — pushed down |
| `urgency.in_progress` | `4.0` | Already-started work gets a "finish it" boost |
| `urgency.notes_count` | `0.5` | Maximum notes contribution. Scales linearly: `0` notes → `0`, `6+` notes → full coefficient |

### Customizing coefficients

```bash
itr config list                              # show every key + current value
itr config get urgency.priority.critical     # one key
itr config set urgency.priority.critical 15  # bump critical priority
itr config set urgency.kind.bug 5            # make bugs more urgent
itr config reset                             # restore every key to its default
```

Overrides live in the `config` table inside `.itr.db`, so they are
project-local and survive across `itr` upgrades.

## Worked Example

Consider this issue, created 5 days ago:

```
ID:42 STATUS:in-progress PRIORITY:high KIND:bug
TAGS:auth,security
FILES:src/auth.rs
TITLE: Fix session token refresh race
ACCEPTANCE: cargo test auth::refresh passes under -j 16
```

Additional context:

- The issue **blocks** issue `#50` (which is still open), so the blocking
  signal fires.
- It is **not** blocked by anything itself.
- It has **3 notes** attached from prior investigation.
- All coefficients are at their defaults.

Computing each component:

| Component | Math | Value |
|-----------|------|-------|
| `priority.high` | direct lookup | `+6.0` |
| `kind.bug` | direct lookup | `+2.0` |
| `blocking` | blocks `#50` | `+8.0` |
| `blocked` | not blocked | `0` (omitted from breakdown) |
| `age` | `2.0 * clamp(5/10, 0, 1)` = `2.0 * 0.5` | `+1.0` |
| `in_progress` | status is `in-progress` | `+4.0` |
| `has_acceptance` | acceptance field non-empty | `+1.0` |
| `notes` | `0.5 * min(3/6, 1)` = `0.5 * 0.5` | `+0.25` |

Sum: `6.0 + 2.0 + 8.0 + 1.0 + 4.0 + 1.0 + 0.25` = **`22.25`**.

`itr get 42 -f json` would emit something like:

```json
{
  "id": 42,
  "urgency": 22.25,
  "urgency_breakdown": [
    ["priority.high", 6.0],
    ["kind.bug", 2.0],
    ["blocking", 8.0],
    ["age", 1.0],
    ["in_progress", 4.0],
    ["has_acceptance", 1.0],
    ["notes", 0.25]
  ]
}
```

### How a single tweak shifts the queue

Suppose the team decides bugs need to leapfrog features more aggressively:

```bash
itr config set urgency.kind.bug 5.0
```

Re-reading the same issue now yields:

| Component | Old | New |
|-----------|-----|-----|
| `kind.bug` | `+2.0` | `+5.0` |
| Total | `22.25` | **`25.25`** |

No data was rewritten — the next `itr ready` invocation recomputes from
current state and the issue moves up the queue. Reverting with
`itr config reset` (or `itr config set urgency.kind.bug 2.0`) takes effect
just as immediately.

## Reference

- Implementation: [`src/urgency.rs`](../src/urgency.rs) — `UrgencyConfig`,
  `compute_urgency`, `compute_urgency_with_breakdown`.
- Storage of overrides: the `config` table; see
  [docs/schema.md](schema.md).
- Soft-fallback philosophy that governs unknown coefficient values:
  [docs/soft_fallbacks.md](soft_fallbacks.md).
