#!/usr/bin/env bash
# Formats the file that was just written, so a diff never mixes real changes
# with whitespace churn. Reads the hook payload on stdin and formats only that
# one file — running the whole workspace formatter on every edit is slow enough
# that it gets turned off, and a formatter that is off is not a convention.
set -euo pipefail

payload=$(cat)
file=$(printf '%s' "$payload" | sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)

[ -z "$file" ] && exit 0
[ -f "$file" ] || exit 0

root="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

case "$file" in
  *.rs)
    command -v rustfmt >/dev/null 2>&1 && rustfmt --edition 2024 "$file" 2>/dev/null || true
    ;;
  *.ts | *.tsx | *.js | *.json | *.css | *.html)
    # parts/*.json is data the loader validates; formatting it keeps diffs to
    # the values that actually changed.
    if [ -x "$root/node_modules/.bin/prettier" ]; then
      "$root/node_modules/.bin/prettier" --write --ignore-unknown "$file" >/dev/null 2>&1 || true
    fi
    ;;
esac

exit 0
