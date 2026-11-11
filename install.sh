#!/bin/sh
# Install LaunchX.
#   macOS: build LaunchX.app into ~/Applications and link lx from it.
#   Linux: build lx and copy it into the bin directory.
# Usage: ./install.sh [bin-directory]   (default ~/.local/bin)
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${1:-"$HOME/.local/bin"}
cd "$project_dir"
mkdir -p "$bin_dir"

if [ "$(uname)" = "Darwin" ]; then
  ./macos/build.sh
  apps="$HOME/Applications"
  mkdir -p "$apps"
  if pgrep -qx LaunchX; then
    osascript -e 'tell application "LaunchX" to quit' >/dev/null 2>&1 || true
    sleep 1
  fi
  rm -rf "$apps/LaunchX.app.new"
  cp -R target/LaunchX.app "$apps/LaunchX.app.new"
  rm -rf "$apps/LaunchX.app"
  mv "$apps/LaunchX.app.new" "$apps/LaunchX.app"
  ln -sfn "$apps/LaunchX.app/Contents/MacOS/lx" "$bin_dir/lx"
  printf 'Installed %s\nLinked %s -> the lx inside the app\n' "$apps/LaunchX.app" "$bin_dir/lx"
else
  cargo build --release
  install -m 0755 target/release/lx "$bin_dir/lx.new"
  mv "$bin_dir/lx.new" "$bin_dir/lx"
  printf 'Installed %s\n' "$bin_dir/lx"
fi

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) printf 'Add %s to your PATH to use lx from any directory.\n' "$bin_dir" ;;
esac
if pgrep -qf "lx daemon" 2>/dev/null; then
  printf 'A launcher is already running from the previous build; restart it with: lx down && lx up\n'
else
  printf 'Next: lx up (or open LaunchX and press Start), then lx add <project>\n'
fi
