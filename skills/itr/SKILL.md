---
name: itr
description: Use the `itr` CLI to file issues into the project's agent-first issue tracker. Trigger ANY time the user asks to file, log, open, create, or track an issue/bug/ticket/task — even if they don't say "itr". Phrases that should fire this skill include "file an issue for X", "log a bug", "open a ticket", "track this as a task", "add this to the backlog", "create an issue", and similar. Also fire when the user asks to bulk-create issues from a list. Do NOT fire for working/claiming/closing existing issues — only for creating new ones.
---

# itr — file issues

`itr` is the project's issue tracker CLI (already on PATH). For filing, you only need `itr add` (and `itr batch add` for bulk).

**Before filing the first issue in a session:**
1. Run `itr stats` to confirm a database exists in this project. If it doesn't, the command will tell you to run `itr init` — surface that to the user and confirm before initializing (creates `.itr.db` in the cwd).
2. Run `itr agent-info` once to get the authoritative usage guide — flags, urgency model, and conventions can change. Use what you learn there over what's written below.
3. **Look up project story conventions** so titles, bodies, AC, and tags mirror how this team writes issues. Check in this priority order:
   - `./STORY_STYLE.md` — the canonical location (built by `/story-style`).
   - `CLAUDE.md` / `AGENTS.md` — scan for sections about story style, issue conventions, ticket format, or "how we write issues". Use anything relevant.
   - If neither exists, fall back to the defaults below and print a single soft-suggest line: `No STORY_STYLE.md found — consider /story-style to capture project conventions.` Do **not** pause; this is a surface, not a gate. Skip the suggest if the user has already declined it this session.

   When a style is found, follow it: title shape, body template, AC format, tag taxonomy, priority scheme, terminology, voice. The style file overrides the defaults in this skill.

## The one command

```
itr add "<title>" -p <priority> -k <kind> -c "<context>" -a "<acceptance>" --tags "<a,b>" --files "<path1,path2>"
```

- **title** — imperative, specific. "Fix race in upload queue" not "upload broken".
- **-p** — `critical | high | medium | low`. Default to `medium` unless the user signals otherwise.
- **-k** — `bug | feature | task | epic`. Pick from what the user described; ask only if genuinely ambiguous.
- **-c / --body** — *why* this matters and any reproduction context. Multi-line is fine.
- **-a** — concrete acceptance criteria. Skip only if the user explicitly said "just a placeholder".
- **--files** — relative paths to files implicated by the issue. High signal for the agent who picks it up.
- **--tags / --skills** — comma-separated. Use existing tags where possible (check with `itr list -f json --fields tags`).
- **--blocked-by / --parent** — set if the user names a dependency or parent epic.

If `ITR_AGENT` is unset in the environment, default to `claude` by prefixing the command: `ITR_AGENT=claude itr add ...`. The audit log uses this for attribution.

## Bulk filing

When the user dumps a list ("file these five things…"), use `itr batch add` with a JSON array on stdin — one round-trip instead of N. Schema mirrors `itr add` flags. See `itr agent-info` for exact field names.

## Workflow

1. **Gather what's missing.** Before filing, make sure you have: a clear title, priority signal, kind, and at least minimal acceptance. If the user gave you a vague ask, ask one focused question rather than filing a stub.
2. **Check for duplicates.** Run `itr search "<key terms>" -f json --fields id,title,status` first. If a match exists, surface it and ask whether to update/relate instead of creating a new one.
3. **File it.** Use `-f json` so you can capture the new ID from the response.
4. **Report back** with the issue ID and a one-line summary. Don't dump the full JSON.

## Don't

- Don't invent priorities, kinds, or acceptance criteria the user didn't imply — ask or leave a field empty.
- Don't use full paths like `~/.cargo/bin/itr`. It's on `$PATH`.
- Don't file the same issue twice. Search first.
- Don't claim/start/close issues from this skill — that's outside scope.
