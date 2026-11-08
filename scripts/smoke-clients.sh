#!/usr/bin/env bash
# Per-client smoke test: does each installed agent client discover a Quome
# skill and reach the `playbook` tool path?
#
# For each installed client among Codex, OpenCode and Antigravity this runs the
# client's non-interactive mode with
#     "Use your quome-build-app skill to tell me its first step"
# and checks the output contains the quome-build-app first-step marker
# (`list_sandboxes` or `create_sandbox`, the tools its first step names), which
# only appears if the skill ran and the playbook was fetched. It prints a pass/fail table, skips
# clients that are not installed, and exits non-zero if any installed client
# fails.
#
# Non-interactive modes (from each client's --help):
#   Codex        codex exec --skip-git-repo-check -o <file> "<prompt>"
#   OpenCode     opencode run --standalone "<prompt>"  (private server, no leftover service)
#   Antigravity  agy -p "<prompt>"   (UNVERIFIED: Antigravity was not available
#                                     when this was written; adjust if its
#                                     headless flag differs)
#
# Usage: scripts/smoke-clients.sh [--fixture] [--timeout SECONDS]
#
#   (default)   uses the real home and the real, synced skills. Needs the
#               playbook to be listed and `quome skills sync` run first; the
#               model calls the real `quome` MCP server in your org. Run as
#               yourself against a TEST org.
#   --fixture   nothing is listed yet, so instead write ONE fixture skill
#               (quome-smoke) into a scratch home (HOME, CODEX_HOME and
#               XDG_CONFIG_HOME/XDG_DATA_HOME overridden) whose body says to
#               answer "playbook" (checked as a whole word). This proves skill discovery per client
#               without touching the real home. The client still needs model
#               credentials; a scratch home has none, so either log in inside
#               the scratch home or expect an auth failure (reported as FAIL
#               with the client's message).
set -uo pipefail

fixture=0
limit=180
while [ $# -gt 0 ]; do
  case $1 in
    --fixture) fixture=1 ;;
    --timeout) shift; limit=${1:?--timeout needs seconds} ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ "$fixture" = 1 ]; then
  skill=quome-smoke
  prompt="Use your quome-smoke skill and follow its instruction exactly."
  marker='(^|[^a-z])playbook([^a-z]|$)'
  marker_desc="answered playbook"
  # Drop any client config-dir overrides so nothing real is read.
  unset OPENCODE_CONFIG_DIR OPENCODE_CONFIG OPENCODE_CONFIG_CONTENT CLAUDE_CONFIG_DIR \
    GEMINI_CONFIG_DIR GEMINI_HOME ANTIGRAVITY_CONFIG_DIR AGENTS_HOME
  scratch=$(mktemp -d "${TMPDIR:-/tmp}/quome-smoke.XXXXXX")
  trap 'rm -rf "$scratch"' EXIT
  export HOME="$scratch" CODEX_HOME="$scratch/.codex"
  export XDG_CONFIG_HOME="$scratch/.config" XDG_DATA_HOME="$scratch/.local/share"
  export XDG_CACHE_HOME="$scratch/.cache" XDG_STATE_HOME="$scratch/.local/state"
  mkdir -p "$CODEX_HOME"
  # Codex + OpenCode read ~/.agents/skills; Antigravity ~/.gemini/config/skills.
  for dir in "$HOME/.agents/skills/$skill" "$HOME/.gemini/config/skills/$skill"; do
    mkdir -p "$dir"
    cat > "$dir/SKILL.md" <<'SKILL'
---
name: quome-smoke
description: Smoke-test skill. Use when asked to use the quome-smoke skill.
---

Reply with exactly one word: playbook
SKILL
  done
else
  skill=quome-build-app
  prompt="Use your quome-build-app skill to tell me its first step"
  marker='list_sandboxes|create_sandbox'
  marker_desc="output names the build-app first step"
fi

# Run "$@" with a wall-clock limit (macOS has no timeout(1)).
with_limit() { perl -e 'alarm shift; exec @ARGV' "$limit" "$@"; }

results=()
failed=0
record() { # name status detail
  results+=("$1|$2|$3")
  [ "$2" = FAIL ] && failed=1
  return 0
}

check() { # name output
  if printf '%s' "$2" | grep -Eqi "$marker"; then
    record "$1" PASS "$marker_desc"
  else
    record "$1" FAIL "$(printf '%s' "$2" | tr '\n' ' ' | cut -c1-120)"
  fi
}

if command -v codex >/dev/null; then
  out="$(mktemp "${TMPDIR:-/tmp}/codex-smoke.XXXXXX")"
  with_limit codex exec --skip-git-repo-check -o "$out" "$prompt" >"$out.log" 2>&1
  rc=$?
  if [ -s "$out" ]; then check codex "$(cat "$out")"; else record codex FAIL "rc=$rc: $(tr '\n' ' ' <"$out.log" | tail -c 120)"; fi
  rm -f "$out" "$out.log"
else
  record codex SKIP "not installed"
fi

if command -v opencode >/dev/null; then
  o="$(with_limit opencode run --standalone "$prompt" 2>&1)"
  rc=$?
  if [ "$rc" = 0 ]; then check opencode "$o"; else record opencode FAIL "rc=$rc: $(printf '%s' "$o" | tr '\n' ' ' | tail -c 120)"; fi
else
  record opencode SKIP "not installed"
fi

ag=$(command -v agy || command -v antigravity || true)
if [ -n "$ag" ]; then
  o="$(with_limit "$ag" -p "$prompt" 2>&1)"
  rc=$?
  if [ "$rc" = 0 ]; then check antigravity "$o"; else record antigravity FAIL "rc=$rc: $(printf '%s' "$o" | tr '\n' ' ' | tail -c 120)"; fi
else
  record antigravity SKIP "not installed"
fi

printf '\n%-13s %-5s %s\n' CLIENT RESULT DETAIL
for r in "${results[@]}"; do
  IFS='|' read -r n s d <<<"$r"
  printf '%-13s %-5s %s\n' "$n" "$s" "$d"
done
exit "$failed"
