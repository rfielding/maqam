#!/usr/bin/env bash
set -euo pipefail

ENV_FILE="${HOME}/.config/claude/env"
BASHRC="${HOME}/.bashrc"

mkdir -p "$(dirname "$ENV_FILE")"

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  read -r -s -p "Enter ANTHROPIC_API_KEY: " api_key
  echo
else
  api_key="${ANTHROPIC_API_KEY}"
fi

if [[ -z "${api_key}" ]]; then
  echo "Error: ANTHROPIC_API_KEY is empty" >&2
  exit 1
fi

cat > "$ENV_FILE" <<EOKEY
export ANTHROPIC_API_KEY='${api_key}'
EOKEY
chmod 600 "$ENV_FILE"

if ! grep -Fq "source ${ENV_FILE}" "$BASHRC" 2>/dev/null; then
  {
    echo
    echo "# Claude API key"
    echo "source ${ENV_FILE}"
  } >> "$BASHRC"
fi

# Load into current shell for immediate use when sourced.
export ANTHROPIC_API_KEY="${api_key}"

if command -v claude >/dev/null 2>&1; then
  echo "Claude CLI: $(claude --version)"
else
  echo "Warning: claude command not found" >&2
fi

echo "Saved key to ${ENV_FILE} and added source line to ${BASHRC}."
echo "Open a new shell or run: source ${ENV_FILE}"
