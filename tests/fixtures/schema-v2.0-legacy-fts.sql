-- Reconstructed on-disk shape of a DB last opened by itr v2.0.0 .. v2.10.0 (open_db at b620fe7):
-- oldest SCHEMA (8704df7) + migrate_add_skills + migrate_add_assigned_to + migrate_add_events
-- + migrate_add_relations (DDL verbatim from v2.0.0) + legacy FTS (content='', content_rowid=id, no triggers).
-- Apply AFTER schema-v1.0-oldest.sql (DDL and sample rows). Keep it historical: never edit it to match
-- the current schema. Note the migrated column order: skills and assigned_to come after updated_at.
ALTER TABLE issues ADD COLUMN skills TEXT NOT NULL DEFAULT '[]';
ALTER TABLE issues ADD COLUMN assigned_to TEXT NOT NULL DEFAULT '';
CREATE TABLE events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                field       TEXT NOT NULL,
                old_value   TEXT NOT NULL DEFAULT '',
                new_value   TEXT NOT NULL DEFAULT '',
                agent       TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );
CREATE INDEX idx_events_issue ON events(issue_id);
CREATE INDEX idx_events_created ON events(created_at);
CREATE TABLE relations (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                source_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                target_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                relation_type   TEXT NOT NULL CHECK(relation_type IN ('duplicate', 'related', 'supersedes')),
                created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                UNIQUE(source_id, target_id, relation_type)
            );
CREATE INDEX idx_relations_source ON relations(source_id);
CREATE INDEX idx_relations_target ON relations(target_id);
CREATE VIRTUAL TABLE IF NOT EXISTS issues_fts USING fts5(
            title, context, acceptance, tags_text, files_text, skills_text, close_reason,
            content='', content_rowid=id
        );

-- v2.x data: an audit event, a relation, and a legacy FTS index that is wrong in the two ways the
-- pre-v2.10.1 design allowed: issue 2 still carries a stale token ("cogwheel") from an old title that
-- a contentless index could not remove, and issue 4 was never indexed (the table started empty).
INSERT INTO events (issue_id, field, old_value, new_value, agent, created_at)
VALUES (2, 'title', 'Build the cogwheel', 'Build the sprocket', 'v2-agent', '2026-03-03T00:00:00Z');
INSERT INTO relations (source_id, target_id, relation_type, created_at) VALUES (4, 2, 'related', '2026-03-03T00:00:00Z');
INSERT INTO issues_fts (rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
VALUES (1, 'Widget epic', 'umbrella for widget work', '', 'widgets', '', '', '');
INSERT INTO issues_fts (rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
VALUES (2, 'Build the cogwheel', 'machined from brass', 'sprocket spins', 'widgets backend', 'src/sprocket.rs', '', '');
INSERT INTO issues_fts (rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
VALUES (3, 'Retire the gadget', '', '', '', '', '', 'gadget retired in v1.1');
