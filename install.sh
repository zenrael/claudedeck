#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ID="dev.project23.claudedeck.sdPlugin"
BUNDLE="$HERE/plugin/$PLUGIN_ID"
TARGET="x86_64-unknown-linux-gnu"
BIN_NAME="claudedeck-$TARGET"
DEST="$HOME/.config/opendeck/plugins/$PLUGIN_ID"

echo "==> building release binary"
cargo build --release

echo "==> staging binary into bundle"
cp "$HERE/target/release/claudedeck" "$BUNDLE/$BIN_NAME"
chmod +x "$BUNDLE/$BIN_NAME"

echo "==> installing bundle to $DEST"
mkdir -p "$DEST/assets" "$DEST/propertyInspector"
cp "$BUNDLE/manifest.json" "$DEST/"
cp "$BUNDLE/assets/"* "$DEST/assets/"
cp "$BUNDLE/propertyInspector/"* "$DEST/propertyInspector/"
# Replace the binary via temp+mv: a rename swaps the directory entry without
# truncating the old inode, so this works even while OpenDeck has the previous
# build running (a plain cp would fail with "Text file busy"). OpenDeck still
# needs a restart to actually run the new build.
cp "$BUNDLE/$BIN_NAME" "$DEST/.$BIN_NAME.new"
chmod +x "$DEST/.$BIN_NAME.new"
mv -f "$DEST/.$BIN_NAME.new" "$DEST/$BIN_NAME"

DATA_DIR="$HOME/.local/share/claudedeck"
echo "==> installing statusline helper to $DATA_DIR"
mkdir -p "$DATA_DIR"
cp "$HERE/scripts/statusline.py" "$DATA_DIR/statusline.py"
chmod +x "$DATA_DIR/statusline.py"
# Scoped settings file: claude is launched with `--settings` pointing here, so the
# statusline writer is enabled only for ClaudeDeck-launched sessions (global
# ~/.claude/settings.json is left untouched).
cat > "$DATA_DIR/session-settings.json" <<EOF
{
  "statusLine": {
    "type": "command",
    "command": "$DATA_DIR/statusline.py",
    "padding": 0
  }
}
EOF

echo "==> done."
echo "    Restart OpenDeck (or reload plugins) to load ClaudeDeck,"
echo "    then drag the 'Claude Session' action onto a key."
