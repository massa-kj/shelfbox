#!/usr/bin/env sh
# install.sh — installer for shelfbox
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/scripts/install.sh | sh
#
# Environment variables:
#   VERSION      — tag to install (e.g. v0.1.0). Defaults to the latest release.
#   INSTALL_DIR  — directory to place the binary. Defaults to ~/.local/bin.
#   LINUX_LIBC   — libc flavor for Linux: musl (default) or gnu.

set -eu

REPO="massa-kj/shelfbox"
BINARY="shelfbox"

# ── Resolve install directory ─────────────────────────────────────────────────

INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# ── Detect OS ─────────────────────────────────────────────────────────────────

case "$(uname -s)" in
    Linux)  OS="linux"  ;;
    Darwin) OS="darwin" ;;
    *)
        echo "error: unsupported OS: $(uname -s)" >&2
        exit 1
        ;;
esac

# ── Detect architecture ───────────────────────────────────────────────────────

case "$(uname -m)" in
    x86_64 | amd64)   ARCH="x86_64"  ;;
    aarch64 | arm64)  ARCH="aarch64" ;;
    *)
        echo "error: unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

# ── Map OS + arch → Rust target triple ───────────────────────────────────────

case "${OS}-${ARCH}" in
    linux-x86_64 | linux-aarch64)
        LINUX_LIBC="${LINUX_LIBC:-musl}"
        case "$LINUX_LIBC" in
            musl | gnu) ;;
            *)
                echo "error: unsupported LINUX_LIBC: $LINUX_LIBC" >&2
                echo "       Expected 'musl' or 'gnu'." >&2
                exit 1
                ;;
        esac
        TARGET="${ARCH}-unknown-linux-${LINUX_LIBC}"
        ;;
    darwin-x86_64)  TARGET="x86_64-apple-darwin"       ;;
    darwin-aarch64) TARGET="aarch64-apple-darwin"      ;;
esac

# ── Resolve version ───────────────────────────────────────────────────────────

if [ -z "${VERSION:-}" ]; then
    echo "Fetching latest release version..."
    VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' \
        | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    if [ -z "$VERSION" ]; then
        echo "error: could not determine the latest release version." >&2
        echo "       Set the VERSION environment variable and try again." >&2
        exit 1
    fi
fi

# ── Download and install ──────────────────────────────────────────────────────

ARCHIVE="${BINARY}-${VERSION}-${TARGET}.tar.gz"
CHECKSUM="${ARCHIVE}.sha256"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

TMPDIR="$(mktemp -d)"
# shellcheck disable=SC2064
trap "rm -rf '$TMPDIR'" EXIT INT TERM

echo "Downloading ${BINARY} ${VERSION} for ${TARGET}..."
curl -fsSL "${BASE_URL}/${ARCHIVE}"  -o "${TMPDIR}/${ARCHIVE}"
curl -fsSL "${BASE_URL}/${CHECKSUM}" -o "${TMPDIR}/${CHECKSUM}"

echo "Verifying checksum..."
# Run from TMPDIR so sha256sum can find the archive by relative name
( cd "$TMPDIR" && sha256sum --check "${CHECKSUM}" --quiet )
echo "Checksum OK."

echo "Extracting..."
tar xzf "${TMPDIR}/${ARCHIVE}" -C "$TMPDIR"

mkdir -p "$INSTALL_DIR"
cp "${TMPDIR}/${BINARY}-${VERSION}-${TARGET}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
chmod +x "${INSTALL_DIR}/${BINARY}"

echo ""
echo "Installed ${BINARY} ${VERSION} → ${INSTALL_DIR}/${BINARY}"

# ── PATH hint ────────────────────────────────────────────────────────────────

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "NOTE: ${INSTALL_DIR} is not in your PATH."
        echo "      Add the following line to your shell profile and reload it:"
        echo ""
        echo "        export PATH=\"${INSTALL_DIR}:\$PATH\""
        echo ""
        ;;
esac
