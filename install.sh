#!/bin/sh
# BeforeRunIt installer.
#
#   curl -fsSL https://github.com/beforerunit/beforerunit/releases/latest/download/install.sh | sh
#
# Options (or environment variables):
#   --prefix DIR       install into DIR/bin and DIR/lib/beforerunit
#                      (BEFORERUNIT_PREFIX; default ~/.local, or /usr/local as root)
#   --version vX.Y.Z   install a specific release (BEFORERUNIT_VERSION; default latest)
#   --shell NAME       hook up bash, zsh or fish (default: your login shell)
#   --no-modify-rc     do not touch shell startup files
#   --no-fonts-hint    do not suggest a Nerd Font
#
# Run from an extracted release tarball, it installs the files next to it
# instead of downloading.

set -eu

REPO="beforerunit/beforerunit"
PREFIX="${BEFORERUNIT_PREFIX:-}"
VERSION="${BEFORERUNIT_VERSION:-latest}"
SHELL_NAME=""
MODIFY_RC=1
FONTS_HINT=1

say() { printf '%s\n' "$*"; }
err() { printf 'beforerunit install: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --prefix) PREFIX=${2:?--prefix needs a directory}; shift 2 ;;
    --version) VERSION=${2:?--version needs a value}; shift 2 ;;
    --shell) SHELL_NAME=${2:?--shell needs bash, zsh or fish}; shift 2 ;;
    --no-modify-rc) MODIFY_RC=0; shift ;;
    --no-fonts-hint) FONTS_HINT=0; shift ;;
    -h | --help) sed -n '2,16p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) err "unknown option: $1" ;;
  esac
done

[ "$(uname -s)" = Linux ] || err "only Linux is supported"

if [ -z "$PREFIX" ]; then
  if [ "$(id -u)" = 0 ]; then PREFIX=/usr/local; else PREFIX="$HOME/.local"; fi
fi
BIN_DIR="$PREFIX/bin"
LIB_DIR="$PREFIX/lib/beforerunit"

case $(uname -m) in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) err "no prebuilt binary for $(uname -m); build from source: cargo install --git https://github.com/$REPO beforerunit" ;;
esac

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    err "needs curl or wget"
  fi
}

SRC=""
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd || true)
if [ -n "$HERE" ] && [ -x "$HERE/beforerunit" ] && [ -f "$HERE/libbeforerunit_bash.so" ]; then
  SRC=$HERE
elif [ -n "$HERE" ] && [ -f "$HERE/Cargo.toml" ] && [ -d "$HERE/crates/beforerunit" ]; then
  # Source checkout: install what cargo built, building it first if needed.
  if [ ! -x "$HERE/target/release/beforerunit" ] || [ ! -f "$HERE/target/release-bash/libbeforerunit_bash.so" ]; then
    command -v cargo >/dev/null 2>&1 || err "this is a source checkout and cargo is not installed. Install Rust from https://rustup.rs, or use a release: curl -fsSL https://github.com/$REPO/releases/latest/download/install.sh | sh"
    say "Building from source (a few minutes the first time)"
    (cd "$HERE" && cargo build --release && cargo build --profile release-bash -p beforerunit-bash) || err "build failed"
  fi
  STAGE=$(mktemp -d)
  trap 'rm -rf "$STAGE"' EXIT INT TERM
  cp "$HERE/target/release/beforerunit" "$HERE/target/release-bash/libbeforerunit_bash.so" "$STAGE/"
  SRC=$STAGE
else
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT INT TERM
  NAME="beforerunit-$ARCH-linux"
  if [ "$VERSION" = latest ]; then
    BASE="https://github.com/$REPO/releases/latest/download"
  else
    BASE="https://github.com/$REPO/releases/download/$VERSION"
  fi
  say "Downloading $NAME ($VERSION)"
  fetch "$BASE/$NAME.tar.gz" "$TMP/$NAME.tar.gz" ||
    err "could not download $BASE/$NAME.tar.gz (is there a published release yet?). To build from source instead: git clone https://github.com/$REPO && cd beforerunit && ./install.sh"
  if command -v sha256sum >/dev/null 2>&1 && fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" 2>/dev/null; then
    (cd "$TMP" && grep " $NAME.tar.gz\$" SHA256SUMS | sha256sum -c --quiet -) || err "checksum mismatch, download is corrupt"
  fi
  tar -C "$TMP" -xzf "$TMP/$NAME.tar.gz"
  SRC="$TMP/$NAME"
fi

mkdir -p "$BIN_DIR" "$LIB_DIR"
install -m 755 "$SRC/beforerunit" "$BIN_DIR/beforerunit.new"
mv -f "$BIN_DIR/beforerunit.new" "$BIN_DIR/beforerunit"
install -m 644 "$SRC/libbeforerunit_bash.so" "$LIB_DIR/libbeforerunit_bash.so.new"
mv -f "$LIB_DIR/libbeforerunit_bash.so.new" "$LIB_DIR/libbeforerunit_bash.so"
ln -sf beforerunit "$BIN_DIR/bri"
"$BIN_DIR/beforerunit" daemon stop >/dev/null 2>&1 || true
say "Installed $("$BIN_DIR/beforerunit" --version) to $BIN_DIR (alias: bri)"

[ -n "$SHELL_NAME" ] || SHELL_NAME=$(basename "${SHELL:-sh}")
BRI="$BIN_DIR/beforerunit"
RC=""
LINE=""
case $SHELL_NAME in
  bash) RC="$HOME/.bashrc"; LINE="eval \"\$('$BRI' init bash)\"" ;;
  zsh) RC="${ZDOTDIR:-$HOME}/.zshrc"; LINE="eval \"\$('$BRI' init zsh)\"" ;;
  fish) RC="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/beforerunit.fish"; LINE="'$BRI' init fish | source" ;;
esac

if [ -z "$RC" ]; then
  say "Your shell ($SHELL_NAME) is not supported yet. BeforeRunIt works with bash, zsh and fish."
elif [ "$MODIFY_RC" = 0 ]; then
  say "Add this line to $RC:"
  say "  $LINE"
elif [ -f "$RC" ] && grep -q "beforerunit' init\|beforerunit init" "$RC"; then
  say "Already hooked up in $RC"
else
  mkdir -p "$(dirname "$RC")"
  printf '\n# BeforeRunIt\n%s\n' "$LINE" >>"$RC"
  say "Hooked up $SHELL_NAME in $RC"
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "Note: $BIN_DIR is not in your PATH. The shell hook works anyway; add it to run 'beforerunit' by name." ;;
esac

if [ "$FONTS_HINT" = 1 ] && command -v fc-list >/dev/null 2>&1 && ! fc-list : family 2>/dev/null | grep -qi "nerd font\| NF"; then
  say "Icons need a Nerd Font. Install Maple Mono NF with: beforerunit fonts install"
fi

say ""
say "Open a new terminal and start typing. Next steps:"
say "  beforerunit setup    AI review, theme and display settings"
say "  beforerunit doctor   check that everything works"
