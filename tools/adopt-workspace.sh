#!/bin/bash
# adopt-workspace.sh — point this project's agent Sessions at its new directory.
#
#   ./tools/adopt-workspace.sh [old-path] [new-path]
#
# ## Why this exists
#
# `mcode --continue` resumes "the latest Session in the current workspace". The workspace is a
# *path*, and the runtime records it in ~/.minimax/v2/sqlite/runtime-state.sqlite in three places:
#
#   1. `local_runtime_sessions.record_json` → `workspaceDir`  (the source of truth, a JSON blob)
#   2. `local_runtime_sessions.workspace_dir` and `.project_workspace_dir`  (projections of it)
#   3. `local_runtime_projects.workspace_dir`  (the workspace's own row)
#
# Move the directory without moving those and every Session for this project becomes unreachable
# from the new path: `-c` finds no workspace, and the workspace row still names a directory that no
# longer exists.
#
# ## When to run it
#
# **After quitting mcode.** While a Session is open the runtime holds its own copy of the record in
# memory and can write it back, which would restore the old path. Run it with mcode closed and the
# change is the last word.
#
# It is safe to run twice: it moves whatever still points at the old path and reports zero when
# there is nothing left to move.
set -euo pipefail

OLD=${1:-/Users/isal/Workspaces/Organizations/BGN/tools/trino_exporter}
NEW=${2:-/Users/isal/Workspaces/Lab/Experiments/query_hive}
DB=${MINIMAX_DB:-"$HOME/.minimax/v2/sqlite/runtime-state.sqlite"}

say() { printf '%s\n' "$*"; }
die() { printf 'adopt-workspace: %s\n' "$*" >&2; exit 1; }

[ -f "$DB" ] || die "no runtime database at $DB"
[ "$OLD" != "$NEW" ] || die "the old and new paths are the same"
case $OLD$NEW in
    *"'"*) die "a path contains a single quote; this script quotes SQL by hand and will not guess" ;;
esac

# A live runtime is the one case that can undo the work below.
if pgrep -x mcode >/dev/null 2>&1 || pgrep -f 'minimax-code/bin/mcode' >/dev/null 2>&1; then
    say "warning: an mcode process is running."
    say "         The runtime keeps a Session's record in memory and can write the old path back."
    say "         Quit mcode and run this again if 'mcode -c' does not resume the Session."
    say ""
fi

STAMP=$(date +%Y%m%d-%H%M%S)
BACKUP="$DB.before-adopt-$STAMP"
# `.backup` rather than `cp`: it takes a consistent snapshot of a database that may have a WAL
# beside it, which a file copy does not.
sqlite3 "$DB" ".backup '$BACKUP'" || die "could not back up $DB"
say "backed up the database to $(basename "$BACKUP")"
say ""

# What was there before, so the output says what actually changed rather than what was attempted.
say "before: sessions on $OLD"
sqlite3 "$DB" "select '  ' || session_id || '  ' || coalesce(title,'(untitled)') from local_runtime_sessions where workspace_dir = '$OLD' or project_workspace_dir = '$OLD' order by updated_at_ms desc limit 40;"
say ""

# Run every update in one transaction: a half-moved mapping (sessions pointing at the new path, the
# workspace row still at the old one) is worse than either end state.
sqlite3 "$DB" <<SQL
BEGIN IMMEDIATE;

update local_runtime_projects
   set workspace_dir = '$NEW', updated_at_ms = cast(strftime('%s','now') as integer) * 1000
 where workspace_dir = '$OLD';

update local_runtime_sessions
   set record_json = json_set(record_json, '\$.workspaceDir', '$NEW')
 where json_extract(record_json, '\$.workspaceDir') = '$OLD';

update local_runtime_sessions
   set workspace_dir = '$NEW'
 where workspace_dir = '$OLD';

update local_runtime_sessions
   set project_workspace_dir = '$NEW'
 where project_workspace_dir = '$OLD';

-- The per-Session memory state names the workspace too, and a pointer at a directory that no longer
-- exists is a memory lookup that silently finds nothing.
update local_runtime_v2_memory_session_states
   set workspace_dir = '$NEW'
 where workspace_dir = '$OLD';

COMMIT;
SQL

say "after:"
printf '  projects moved        : '
sqlite3 "$DB" "select count(*) from local_runtime_projects where workspace_dir = '$NEW';"
printf '  sessions at new path  : '
sqlite3 "$DB" "select count(*) from local_runtime_sessions where workspace_dir = '$NEW';"
printf '  sessions still at old : '
sqlite3 "$DB" "select count(*) from local_runtime_sessions where workspace_dir = '$OLD';"
printf '  records still at old  : '
sqlite3 "$DB" "select count(*) from local_runtime_sessions where json_extract(record_json, '\$.workspaceDir') = '$OLD';"
say ""

say "the most recent Session in the new workspace is now:"
sqlite3 "$DB" "select '  ' || session_id || '  ' || coalesce(title,'(untitled)') from local_runtime_sessions where workspace_dir = '$NEW' order by updated_at_ms desc limit 1;"
say ""
say "next: cd $NEW && mcode -c"
say "if it does not resume, quit mcode and run this script once more (the runtime may have"
say "written its in-memory copy back on exit)."
