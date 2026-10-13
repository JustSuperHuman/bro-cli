#!/bin/sh
# bro installer for macOS and Linux.
#   curl -fsSL https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/install.sh | sh
# Env: BRO_VERSION (tag, default latest), BRO_INSTALL_DIR (default ~/.local/bin),
#      BRO_TOOLS (Quick Install too, e.g. "justtools git node claude", "recommended", "all" or "none"; default: ask).
set -eu

repo="JustSuperHuman/bro-cli"
dir="${BRO_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64|Linux-amd64) target="x86_64-unknown-linux-gnu" ;;
  Darwin-arm64|Darwin-aarch64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  *) echo "bro: no prebuilt binary for $(uname -s) $(uname -m); build from source: https://github.com/$repo#build-from-source" >&2; exit 1 ;;
esac

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
  fetch_out() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
  fetch_out() { wget -qO- "$1"; }
else
  echo "bro: need curl or wget" >&2; exit 1
fi

tag="${BRO_VERSION:-}"
if [ -z "$tag" ]; then
  tag=$(fetch_out "https://api.github.com/repos/$repo/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
  [ -n "$tag" ] || { echo "bro: could not find the latest release" >&2; exit 1; }
fi
name="bro-$tag-$target"
base="https://github.com/$repo/releases/download/$tag"
echo "Installing bro $tag to $dir"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fetch "$base/$name.tar.gz" "$tmp/$name.tar.gz"
fetch "$base/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt"

want=$(awk -v f="$name.tar.gz" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS.txt")
[ -n "$want" ] || { echo "bro: no checksum for $name.tar.gz" >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
  got=$(sha256sum "$tmp/$name.tar.gz" | awk '{print $1}')
else
  got=$(shasum -a 256 "$tmp/$name.tar.gz" | awk '{print $1}')
fi
[ "$want" = "$got" ] || { echo "bro: checksum mismatch for $name.tar.gz" >&2; exit 1; }

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dir"
# install via temp + rename so a running bro is not clobbered mid-write
cp "$tmp/$name/bro" "$dir/.bro.new"
chmod 755 "$dir/.bro.new"
mv -f "$dir/.bro.new" "$dir/bro"

echo "bro $tag installed to $dir/bro"

# Offer Quick Install (JustTools, Git, Node.js, Bun, Python, .NET, the coding agents…); releases before `bro install` skip this.
# curl | sh feeds this script on stdin, so the question is asked on the terminal.
if "$dir/bro" --help 2>/dev/null | grep -q 'bro install'; then
  tools=$(printf '%s' "${BRO_TOOLS:-}" | tr ',' ' ')
  if [ "$tools" = "none" ]; then
    :
  elif [ -n "$tools" ]; then
    # shellcheck disable=SC2086
    "$dir/bro" install $tools || true
  elif [ -r /dev/tty ] && [ -w /dev/tty ]; then
    echo
    "$dir/bro" install </dev/tty || true
  fi
fi
case ":$PATH:" in
  *":$dir:"*) echo "Run: bro" ;;
  *) echo "Add $dir to your PATH, e.g.:"
     echo "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.profile"
     echo "Then open a new terminal and run: bro" ;;
esac
