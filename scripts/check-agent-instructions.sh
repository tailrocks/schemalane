#!/usr/bin/env bash
set -euo pipefail

status=0

for required_file in ./AGENTS.md ./CLAUDE.md; do
  if [[ ! -f "$required_file" ]]; then
    echo "missing required root instruction file: $required_file" >&2
    status=1
  fi
done

while IFS= read -r -d '' agents_file; do
  directory=$(dirname "$agents_file")
  claude_file="$directory/CLAUDE.md"
  lines=$(wc -l < "$agents_file")

  if ((lines > 200)); then
    echo "$agents_file exceeds 200 lines ($lines)" >&2
    status=1
  fi

  if [[ ! -L "$claude_file" ]]; then
    echo "$agents_file has no sibling CLAUDE.md symlink" >&2
    status=1
  elif [[ $(readlink "$claude_file") != "AGENTS.md" ]]; then
    echo "$claude_file must link to sibling AGENTS.md" >&2
    status=1
  elif [[ ! -f "$claude_file" ]]; then
    echo "$claude_file is a dangling symlink" >&2
    status=1
  fi
done < <(
  find . \
    -path './.git' -prune -o \
    -path './target' -prune -o \
    -name AGENTS.md -print0
)

while IFS= read -r -d '' claude_file; do
  agents_file="$(dirname "$claude_file")/AGENTS.md"
  lines=$(wc -l < "$claude_file")

  if ((lines > 200)); then
    echo "$claude_file exceeds 200 lines ($lines)" >&2
    status=1
  fi

  if [[ ! -f "$agents_file" ]]; then
    echo "$claude_file links to a missing sibling AGENTS.md" >&2
    status=1
  fi
done < <(
  find . \
    -path './.git' -prune -o \
    -path './target' -prune -o \
    -name CLAUDE.md -print0
)

exit "$status"
