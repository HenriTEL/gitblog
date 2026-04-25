#!/usr/bin/env bash
set -euo pipefail

AGENTS_MAX_TOKENS="${AGENTS_MAX_TOKENS:-600}"

if [ ! -f AGENTS.md ]; then
  echo "AGENTS token check skipped: AGENTS.md not found."
  exit 0
fi

words=$(wc -w < AGENTS.md)
tokens=$((words * 13 / 10))

if [ "$tokens" -le "$AGENTS_MAX_TOKENS" ]; then
  echo "Estimated $tokens tokens in AGENTS.md"
else
  echo "::error::Estimated $tokens tokens in AGENTS.md exceeds maximum $AGENTS_MAX_TOKENS"
  exit 1
fi
