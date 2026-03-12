#!/usr/bin/env sh
# Wheelhouse installer — curl -sSf https://install.wheelhouse.dev | sh
# Downloads the latest wh binary for the current platform from GitHub Releases.
# SR-01: SLSA verification (best-effort) + binary collision warning.

set -eu

REPO="Wheelhouse-Paris/wheelhouse"
INSTALL_DIR="${WH_INSTALL_DIR:-$HOME/.local/bin}"

# --- Platform detection ---

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)
            case "$arch" in
                x86_64)  echo "x86_64-unknown-linux-musl" ;;
                *)       echo "error: unsupported Linux architecture: $arch" >&2; exit 1 ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                arm64)   echo "aarch64-apple-darwin" ;;
                # Intel Mac: not in MVP release matrix; show clear error
                x86_64)  echo "error: macOS Intel (x86_64) is not supported in MVP. Use macOS ARM (Apple Silicon)." >&2; exit 1 ;;
                *)       echo "error: unsupported macOS architecture: $arch" >&2; exit 1 ;;
            esac
            ;;
        *)
            echo "error: unsupported OS: $os" >&2
            exit 1
            ;;
    esac
}

# --- Download ---

download_binary() {
    target="$1"
    binary_name="wh-${target}"
    url="https://github.com/${REPO}/releases/latest/download/${binary_name}"
    TMPFILE="$(mktemp)"

    echo "Downloading Wheelhouse for ${target}..."
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$TMPFILE" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$TMPFILE" "$url"
    else
        echo "error: curl or wget required" >&2
        rm -f "$TMPFILE"
        exit 1
    fi
}

# --- SLSA verification (best-effort, SR-01) ---

verify_provenance() {
    if command -v gh >/dev/null 2>&1; then
        echo "Verifying SLSA provenance attestation..."
        if gh attestation verify "$TMPFILE" --owner Wheelhouse-Paris 2>/dev/null; then
            echo "SLSA provenance verified."
        else
            echo "warning: SLSA verification failed or attestation not found. Proceeding anyway." >&2
        fi
    else
        echo "note: install 'gh' CLI to enable SLSA provenance verification." >&2
    fi
}

# --- Binary collision warning (SR-01) ---

check_collision() {
    existing="$(command -v wh 2>/dev/null || true)"
    if [ -n "$existing" ] && [ "$existing" != "${INSTALL_DIR}/wh" ]; then
        echo "warning: existing 'wh' found at ${existing}" >&2
        echo "  The new installation at ${INSTALL_DIR}/wh may shadow or be shadowed by it." >&2
    fi
}

# --- Install ---

install_binary() {
    mkdir -p "$INSTALL_DIR"
    mv "$TMPFILE" "${INSTALL_DIR}/wh"
    chmod +x "${INSTALL_DIR}/wh"

    # PATH hint
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo ""
            echo "Add to your shell profile:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac
}

# --- Main ---

main() {
    target="$(detect_target)"
    download_binary "$target"
    verify_provenance
    check_collision
    install_binary

    echo ""
    echo "Wheelhouse installed successfully!"
    "${INSTALL_DIR}/wh" --version
}

main
