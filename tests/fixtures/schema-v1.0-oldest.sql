-- Oldest itr schema: SCHEMA const from commit 8704df7 (feat: init, 2026-02-14), first shipped in v1.0.0.
-- Unchanged through v1.1.0 (38247d3). Extracted verbatim from: git show 8704df7:src/db.rs
-- `itr init` in v1.0.0/v1.1.0 was exactly `execute_batch(SCHEMA)`, so this is a real on-disk shape:
-- no skills/assigned_to columns, no events/relations tables, no FTS, PRAGMA user_version = 0.
--
-- Used by the old-release upgrade tests (tests/integration.sh "schema migrations" section and the
-- `oldest_release_fixture_*` unit tests in src/db.rs). Keep it byte-for-byte historical: never edit
-- the DDL below to match the current schema. The sample rows at the end must keep `itr doctor` clean.
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS issues (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    title           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'in-progress', 'done', 'wontfix')),
    priority        TEXT NOT NULL DEFAULT 'medium'
                    CHECK (priority IN ('critical', 'high', 'medium', 'low')),
    kind            TEXT NOT NULL DEFAULT 'task'
                    CHECK (kind IN ('bug', 'feature', 'task', 'epic')),
    context         TEXT NOT NULL DEFAULT '',
    files           TEXT NOT NULL DEFAULT '[]',
    tags            TEXT NOT NULL DEFAULT '[]',
    acceptance      TEXT NOT NULL DEFAULT '',
    parent_id       INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    close_reason    TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS dependencies (
    blocker_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    blocked_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id != blocked_id)
);

CREATE TABLE IF NOT EXISTS notes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id        INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    content         TEXT NOT NULL,
    agent           TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS config (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_issues_status ON issues(status);
CREATE INDEX IF NOT EXISTS idx_issues_priority ON issues(priority);
CREATE INDEX IF NOT EXISTS idx_issues_kind ON issues(kind);
CREATE INDEX IF NOT EXISTS idx_issues_parent ON issues(parent_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocked ON dependencies(blocked_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocker ON dependencies(blocker_id);
CREATE INDEX IF NOT EXISTS idx_notes_issue ON notes(issue_id);

CREATE TRIGGER IF NOT EXISTS trg_issues_updated_at
    AFTER UPDATE ON issues
    FOR EACH ROW
BEGIN
    UPDATE issues SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
    WHERE id = OLD.id;
END;

-- Sample data as a v1.x user would have it. updated_at is backdated so a test can observe
-- trg_issues_updated_at firing after the upgrade.
INSERT INTO issues (id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at)
VALUES (1, 'Widget epic', 'open', 'high', 'epic', 'umbrella for widget work', '[]', '["widgets"]', '', NULL, '', '2026-02-15T00:00:00Z', '2026-02-15T00:00:00Z');
INSERT INTO issues (id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at)
VALUES (2, 'Build the sprocket', 'open', 'medium', 'task', 'machined from brass', '["src/sprocket.rs"]', '["widgets","backend"]', 'sprocket spins', 1, '', '2026-02-15T00:00:00Z', '2026-02-15T00:00:00Z');
INSERT INTO issues (id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at)
VALUES (3, 'Retire the gadget', 'done', 'low', 'bug', '', '[]', '[]', '', NULL, 'gadget retired in v1.1', '2026-02-15T00:00:00Z', '2026-02-15T00:00:00Z');
INSERT INTO issues (id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at)
VALUES (4, 'Polish the manual', 'open', 'medium', 'feature', '', '[]', '["docs"]', '', 1, '', '2026-02-15T00:00:00Z', '2026-02-15T00:00:00Z');
INSERT INTO dependencies (blocker_id, blocked_id, created_at) VALUES (2, 4, '2026-02-15T00:00:00Z');
INSERT INTO notes (issue_id, content, agent, created_at) VALUES (2, 'tolerance checked by calipers', 'v1-agent', '2026-02-15T00:00:00Z');
INSERT INTO config (key, value) VALUES ('urgency.priority.high', '7');
