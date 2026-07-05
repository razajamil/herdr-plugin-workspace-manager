#!/usr/bin/env sh
# install.sh — put the `herdr-workspace-manager` CLI on your PATH.
#
# The CLI is only needed for `remove-gone` (cleaning up merged worktrees);
# layouts work without it. The plugin itself is installed separately with
# `herdr plugin install razajamil/herdr-plugin-workspace-manager`.
#
# Usage:
#   ./install.sh [BIN_DIR]            # BIN_DIR defaults to ~/.local/bin
#   curl -fsSL <raw-url>/install.sh | sh
set -eu

PLUGIN_ID="herdr-plugin-workspace-manager"
BIN_NAME="herdr-workspace-manager"
BIN_DIR="${1:-$HOME/.local/bin}"

# Locate the plugin: prefer this script's own directory (running from a clone),
# else ask herdr where the installed/linked plugin lives (so `curl | sh` works).
root=""
script_dir=$(cd "$(dirname "$0")" 2>/dev/null && pwd) || script_dir=""
if [ -n "$script_dir" ] && [ -f "$script_dir/bin/$BIN_NAME" ]; then
  root="$script_dir"
elif command -v herdr >/dev/null 2>&1; then
  root=$(herdr plugin list --plugin "$PLUGIN_ID" --json 2>/dev/null \
    | sed -n 's/.*"plugin_root"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1) || root=""
fi

if [ -z "$root" ] || [ ! -f "$root/bin/$BIN_NAME" ]; then
  echo "error: couldn't locate the $PLUGIN_ID plugin." >&2
  echo "Install it first, then re-run this script:" >&2
  echo "  herdr plugin install razajamil/$PLUGIN_ID" >&2
  exit 1
fi

# Build the binary now so the first plugin event / CLI call doesn't pay for it.
sh "$root/bin/$BIN_NAME" --help >/dev/null

mkdir -p "$BIN_DIR"
ln -sf "$root/bin/$BIN_NAME" "$BIN_DIR/$BIN_NAME"
echo "✓ linked $BIN_NAME -> $root/bin/$BIN_NAME ($BIN_DIR)"

case ":$PATH:" in
  *":$BIN_DIR:"*) echo "  $BIN_DIR is on your PATH — run: $BIN_NAME --help" ;;
  *) echo "  note: $BIN_DIR is not on your PATH; add it, e.g.:" >&2
     echo "    export PATH=\"$BIN_DIR:\$PATH\"" >&2 ;;
esac
