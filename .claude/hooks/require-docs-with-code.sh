#!/usr/bin/env bash
# PreToolUse(Bash) gate: no shipping code without its documentation.
#
# Blocks `git push` and `gh pr create` when the outgoing change touches repo
# files but no documentation. Documentation that lags the code is worse than
# none — people trust it and act on stale rules.
#
# Escape hatch: prefix the command with DOCS_EXEMPT=1 (e.g. a revert, a CI-only
# fix, a security patch). Deliberate and visible in the transcript, rather than
# silently working around the gate.
#
# Fails OPEN. A hook that cannot determine the diff must not block every push in
# the repo; this is a guardrail, not a tripwire.
set -uo pipefail

payload=$(cat)
cmd=$(printf '%s' "$payload" | jq -r '.tool_input.command // ""' 2>/dev/null) || exit 0

allow() { exit 0; }

# Only gate the two commands that publish work outward.
case "$cmd" in
*"git push"* | *"gh pr create"*) ;;
*) allow ;;
esac

# Deliberate opt-out.
case "$cmd" in *DOCS_EXEMPT=1*) allow ;; esac

# Branch deletions carry no diff.
case "$cmd" in *--delete* | *" :"*) allow ;; esac

git rev-parse --git-dir >/dev/null 2>&1 || allow

# Compare against the branch this repo actually develops on.
base=""
for candidate in origin/develop origin/main origin/master; do
	if git rev-parse --verify --quiet "$candidate" >/dev/null 2>&1; then
		base="$candidate"
		break
	fi
done
[ -n "$base" ] || allow

merge_base=$(git merge-base "$base" HEAD 2>/dev/null) || allow
[ -n "$merge_base" ] || allow

changed=$(git diff --name-only "$merge_base" HEAD 2>/dev/null) || allow
[ -n "$changed" ] || allow # nothing outgoing

# `docs/`, markdown anywhere, and the agent instruction files all count — EXCEPT
# files that regenerate themselves. `module_dependency_map.md` is derived from
# imports and rewritten on commit, so it rides along with code changes and would
# satisfy this gate without a human having documented anything.
DOC_RE='(^docs/|\.md$|\.mdx$|^README|^CHANGELOG)'
GENERATED_RE='^docs/agent_context/module_dependency_map\.md$'

docs=$(printf '%s\n' "$changed" | grep -Ei "$DOC_RE" | grep -Ev "$GENERATED_RE" || true)
[ -z "$docs" ] || allow # real documentation is present — ship it

non_docs=$(printf '%s\n' "$changed" | grep -vEi "$DOC_RE" || true)
[ -n "$non_docs" ] || allow # nothing but the generated file

count=$(printf '%s\n' "$non_docs" | grep -c . || true)
sample=$(printf '%s\n' "$non_docs" | head -5 | sed 's/^/  - /')

reason=$(
	cat <<EOF
BLOQUEADO: este cambio toca $count archivo(s) del repo y ningún documento.

$sample

La documentación se actualiza en el MISMO cambio que el código, nunca después.
Antes de reintentar:

  1. Actualizá la guía o referencia que describe lo que cambiaste.
  2. Grepeá docs/ por referencias obsoletas FUERA del área que tocaste — al
     renombrar o eliminar algo público (constante, env var, tipo de evento,
     campo de config, límite), buscá el nombre viejo Y la prosa que describe el
     comportamiento viejo.
  3. Si el cambio es visible para un consumidor, escribí su nota de migración.

Si este cambio legítimamente no necesita docs (revert, fix de CI, parche de
seguridad), reintentá con el prefijo DOCS_EXEMPT=1.
EOF
)

jq -n --arg r "$reason" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    permissionDecision: "deny",
    permissionDecisionReason: $r
  }
}'
exit 0
