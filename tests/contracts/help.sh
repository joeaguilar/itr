#!/usr/bin/env bash
# tests/contracts/help.sh
#
# Help / no-database output contract (issue #141).
#
# The no-database surface — top-level help, every command and alias --help,
# nested batch/bulk/config/skill help, agent-info, getting-started, schema,
# and the skill command — is part of itr's public contract: agents rely on it
# BEFORE a database exists. This area snapshots the current bytes of all of
# those so accidental drift (a renamed flag, a reordered command, an added
# event/relation table, reworded onboarding text) is caught in review.
#
# Auto-discovered and sourced by tests/integration.sh. Standalone iteration:
#   ITR=./target/release/itr bash tests/contracts/help.sh
# Regenerate just this area's baselines:
#   UPDATE_SNAPSHOTS=1 ITR=./target/release/itr bash tests/contracts/help.sh
#
# Sources _lib.sh relative to its own location, so cwd does not matter.

CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tests/contracts/_lib.sh
. "$CONTRACT_DIR/_lib.sh"

echo ""
echo "--- contract: help (no-database / help output) ---"

# ──────────────────────────────────────────────────────────────────────────
# 0) Version + root help. The version normalization (itr X.Y.Z) is exercised
#    by --version; root --help enumerates the full command surface in order.
# ──────────────────────────────────────────────────────────────────────────
snapshot help version    -- --version
snapshot help root_help  -- --help

# ──────────────────────────────────────────────────────────────────────────
# 1) --help for EVERY top-level command. If a command, flag, default, or
#    description changes, exactly one of these snapshots drifts.
# ──────────────────────────────────────────────────────────────────────────
snapshot help cmd_init_help        -- init --help
snapshot help cmd_add_help         -- add --help
snapshot help cmd_list_help        -- list --help
snapshot help cmd_get_help         -- get --help
snapshot help cmd_update_help      -- update --help
snapshot help cmd_close_help       -- close --help
snapshot help cmd_note_help        -- note --help
snapshot help cmd_note_delete_help -- note-delete --help
snapshot help cmd_note_update_help -- note-update --help
snapshot help cmd_depend_help      -- depend --help
snapshot help cmd_undepend_help    -- undepend --help
snapshot help cmd_next_help        -- next --help
snapshot help cmd_ready_help       -- ready --help
snapshot help cmd_batch_help       -- batch --help
snapshot help cmd_bulk_help        -- bulk --help
snapshot help cmd_graph_help       -- graph --help
snapshot help cmd_stats_help       -- stats --help
snapshot help cmd_summary_help     -- summary --help
snapshot help cmd_export_help      -- export --help
snapshot help cmd_import_help      -- import --help
snapshot help cmd_doctor_help      -- doctor --help
snapshot help cmd_ui_help          -- ui --help
snapshot help cmd_config_help      -- config --help
snapshot help cmd_agent_info_help  -- agent-info --help
snapshot help cmd_skill_help       -- skill --help
snapshot help cmd_schema_help      -- schema --help
snapshot help cmd_upgrade_help     -- upgrade --help
snapshot help cmd_claim_help       -- claim --help
snapshot help cmd_assign_help      -- assign --help
snapshot help cmd_unassign_help    -- unassign --help
snapshot help cmd_log_help         -- log --help
snapshot help cmd_relate_help      -- relate --help
snapshot help cmd_unrelate_help    -- unrelate --help
snapshot help cmd_reindex_help     -- reindex --help
snapshot help cmd_search_help      -- search --help
snapshot help cmd_wip_help         -- wip --help
snapshot help cmd_show_help        -- show --help
snapshot help cmd_help_help        -- help --help

# ──────────────────────────────────────────────────────────────────────────
# 2) --help for EVERY visible alias. Aliases are part of the contract too;
#    they render their target command's help under the invoked alias name.
#      create -> add, deps -> depend, getting-started -> agent-info,
#      start -> claim, current -> wip
# ──────────────────────────────────────────────────────────────────────────
snapshot help alias_create_help          -- create --help
snapshot help alias_deps_help            -- deps --help
snapshot help alias_getting_started_help -- getting-started --help
snapshot help alias_start_help           -- start --help
snapshot help alias_current_help         -- current --help

# ──────────────────────────────────────────────────────────────────────────
# 3) Nested help for batch / bulk / config / skill ACTIONS (per acceptance
#    criteria). These subcommand help screens describe the JSON/stdin and
#    filter contracts agents depend on.
# ──────────────────────────────────────────────────────────────────────────
snapshot help batch_add_help    -- batch add --help
snapshot help batch_close_help  -- batch close --help
snapshot help batch_update_help -- batch update --help
snapshot help batch_note_help   -- batch note --help

snapshot help bulk_close_help   -- bulk close --help
snapshot help bulk_update_help  -- bulk update --help

snapshot help config_list_help  -- config list --help
snapshot help config_get_help   -- config get --help
snapshot help config_set_help   -- config set --help
snapshot help config_reset_help -- config reset --help

snapshot help skill_install_help -- skill install --help
snapshot help skill_path_help    -- skill path --help

# ──────────────────────────────────────────────────────────────────────────
# 4) The no-database INFORMATIONAL commands themselves (not just their --help).
#    These run without an existing DB and form the agent onboarding contract:
#      - agent-info (compact + json)
#      - getting-started alias (one word) and the two-word `getting started`
#        argv-folding alias, both of which dispatch to agent-info
#      - schema (compact + json) — captures the events/relations tables added
#        by migrations as accepted baseline
#      - skill (compact + json) — emits the Claude Code SKILL.md body
# ──────────────────────────────────────────────────────────────────────────
snapshot help agent_info               -- agent-info
snapshot help agent_info_json          -- agent-info -f json
snapshot help getting_started          -- getting-started
snapshot help getting_started_two_word -- getting started
snapshot help schema                   -- schema
snapshot help schema_json              -- schema -f json
snapshot help skill                    -- skill
snapshot help skill_json               -- skill -f json
