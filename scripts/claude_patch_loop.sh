#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <branch-name> <prompt-file>"
  echo "Example: $0 claude/fix-parser prompts/fix_parser.md"
  exit 1
fi

branch="$1"
prompt_file="$2"
patch_file="/tmp/claude_$(date +%Y%m%d_%H%M%S).patch"

if ! command -v claude >/dev/null 2>&1; then
  echo "Error: claude CLI not found" >&2
  exit 1
fi

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  echo "Error: ANTHROPIC_API_KEY is not set" >&2
  echo "Run: source ~/.config/claude/env" >&2
  exit 1
fi

if [[ ! -f "$prompt_file" ]]; then
  echo "Error: prompt file not found: $prompt_file" >&2
  exit 1
fi

current_branch="$(git branch --show-current)"
if [[ -z "$current_branch" ]]; then
  echo "Error: not on a git branch" >&2
  exit 1
fi

if [[ "$current_branch" != "$branch" ]]; then
  git switch -c "$branch" 2>/dev/null || git switch "$branch"
fi

echo "Requesting patch from Claude..."
cat "$prompt_file" | claude -p > "$patch_file"

echo "Patch saved to: $patch_file"

echo "Validating patch..."
git apply --check "$patch_file"

echo "Applying patch..."
git apply "$patch_file"

echo
git status --short
echo

echo "Done. Review changes, run tests, then commit."
