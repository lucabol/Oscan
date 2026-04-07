#!/usr/bin/env sh
set -eu

SOURCE_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
INSTALL_DIR="$HOME/.local/oscan"
BIN_DIR="$HOME/.local/bin"
CREATE_LINK=1

while [ "$#" -gt 0 ]; do
    case "$1" in
        --source-dir)
            SOURCE_DIR="$2"
            shift 2
            ;;
        --install-dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        --bin-dir)
            BIN_DIR="$2"
            shift 2
            ;;
        --no-bin-link)
            CREATE_LINK=0
            shift
            ;;
        *)
            echo "usage: $0 [--source-dir <path>] [--install-dir <path>] [--bin-dir <path>] [--no-bin-link]" >&2
            exit 1
            ;;
    esac
done

if [ ! -f "$SOURCE_DIR/oscan" ]; then
    echo "source bundle must contain an oscan binary" >&2
    exit 1
fi

case "$INSTALL_DIR" in
    ""|"/"|"$HOME"|".")
        echo "refusing to install into unsafe directory '$INSTALL_DIR'" >&2
        exit 1
        ;;
esac

rm -rf "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
cp -RP "$SOURCE_DIR"/. "$INSTALL_DIR"/
chmod +x "$INSTALL_DIR/oscan"
if [ -f "$INSTALL_DIR/install.sh" ]; then
    chmod +x "$INSTALL_DIR/install.sh"
fi

if [ "$CREATE_LINK" -eq 1 ]; then
    mkdir -p "$BIN_DIR"
    ln -sfn "$INSTALL_DIR/oscan" "$BIN_DIR/oscan"
fi

echo "Installed Oscan to $INSTALL_DIR"
if [ -d "$INSTALL_DIR/toolchain" ]; then
    echo "Bundled toolchain installed next to oscan."
fi
if [ "$CREATE_LINK" -eq 1 ]; then
    echo "Symlink refreshed at $BIN_DIR/oscan"
else
    echo "Add $INSTALL_DIR to PATH to run oscan globally."
fi
