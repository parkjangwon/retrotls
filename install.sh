#!/bin/sh
# RetroTLS Installer
# Usage: curl -fsSL https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
REPO="parkjangwon/retrotls"
BINARY_NAME="retrotls"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# Detect OS and architecture
detect_platform() {
    local _os=""
    local _arch=""
    local _ext=""

    case "$(uname -s)" in
        Linux*)     _os="linux";;
        Darwin*)    _os="macos";;
        CYGWIN*|MINGW*|MSYS*) _os="windows";;
        *)          _os="unknown";;
    esac

    case "$(uname -m)" in
        x86_64|amd64)   _arch="x86_64";;
        arm64|aarch64)  _arch="aarch64";;
        *)              _arch="unknown";;
    esac

    if [ "$_os" = "windows" ]; then
        _ext=".zip"
    else
        _ext=".tar.gz"
    fi

    if [ "$_os" = "macos" ] && [ "$_arch" = "aarch64" ]; then
        echo "${BINARY_NAME}-macos-aarch64${_ext}"
    elif [ "$_os" = "macos" ] && [ "$_arch" = "x86_64" ]; then
        echo "${BINARY_NAME}-macos-x86_64${_ext}"
    elif [ "$_os" = "linux" ] && [ "$_arch" = "x86_64" ]; then
        echo "${BINARY_NAME}-linux-x86_64${_ext}"
    elif [ "$_os" = "windows" ]; then
        echo "${BINARY_NAME}-windows-x86_64${_ext}"
    else
        echo "${RED}Error: Unsupported platform $_os/$_arch${NC}" >&2
        exit 1
    fi
}

# Get latest version
get_latest_version() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | \
        grep '"tag_name":' | \
        sed -E 's/.*"tag_name": "([^"]+)".*/\1/'
}

# Download and install
download_and_install() {
    local _version="$1"
    local _asset="$2"
    local _tmp_dir="$(mktemp -d)"
    local _download_url="https://github.com/${REPO}/releases/download/${_version}/${_asset}"

    echo "${BLUE}→${NC} Downloading ${_asset}..."
    
    if ! curl -fsSL "${_download_url}" -o "${_tmp_dir}/${_asset}"; then
        echo "${RED}Error: Failed to download ${_download_url}${NC}" >&2
        rm -rf "${_tmp_dir}"
        exit 1
    fi

    echo "${BLUE}→${NC} Extracting..."
    cd "${_tmp_dir}"
    
    case "${_asset}" in
        *.tar.gz)
            tar -xzf "${_asset}"
            ;;
        *.zip)
            unzip -q "${_asset}"
            ;;
    esac

    # Create install directory if needed
    mkdir -p "${INSTALL_DIR}"

    # Check if binary already exists
    if [ -f "${INSTALL_DIR}/${BINARY_NAME}" ]; then
        echo "${YELLOW}!${NC} Existing installation found. Updating..."
        rm -f "${INSTALL_DIR}/${BINARY_NAME}"
    fi

    # Install binary
    local _binary_name="${BINARY_NAME}"
    if [ "${_asset}" = "${BINARY_NAME}-windows-x86_64.zip" ]; then
        _binary_name="${BINARY_NAME}.exe"
    fi

    mv "${_binary_name}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    # Cleanup
    rm -rf "${_tmp_dir}"

    echo "${GREEN}✓${NC} RetroTLS ${_version} installed to ${INSTALL_DIR}/${BINARY_NAME}"
}

# Check if install directory is in PATH
check_path() {
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*)
            return 0
            ;;
    esac
    return 1
}

# Main install function
install_retrotls() {
    echo "${BLUE}╔════════════════════════════════════╗${NC}"
    echo "${BLUE}║${NC}      ${GREEN}RetroTLS Installer${NC}           ${BLUE}║${NC}"
    echo "${BLUE}╚════════════════════════════════════╝${NC}"
    echo ""

    # Detect platform
    local _asset
    _asset="$(detect_platform)"
    echo "${BLUE}→${NC} Detected platform: ${_asset}"

    # Get latest version
    local _version
    _version="$(get_latest_version)"
    if [ -z "${_version}" ]; then
        echo "${RED}Error: Could not determine latest version${NC}" >&2
        exit 1
    fi
    echo "${BLUE}→${NC} Latest version: ${_version}"

    # Download and install
    download_and_install "${_version}" "${_asset}"

    # Check PATH
    if ! check_path; then
        echo ""
        echo "${YELLOW}!${NC} ${INSTALL_DIR} is not in your PATH"
        echo "   Add this to your shell profile:"
        echo "   ${BLUE}export PATH=\"\$HOME/.local/bin:\$PATH\"${NC}"
    fi

    # Verify installation
    if command -v "${BINARY_NAME}" >/dev/null 2>&1; then
        echo ""
        echo "${GREEN}✓${NC} Installation verified:"
        "${BINARY_NAME}" --version
    fi

    echo ""
    echo "${GREEN}Done!${NC} Run '${BINARY_NAME} --help' to get started."
}

# Uninstall function
uninstall_retrotls() {
    echo "${BLUE}╔════════════════════════════════════╗${NC}"
    echo "${BLUE}║${NC}      ${RED}RetroTLS Uninstaller${NC}         ${BLUE}║${NC}"
    echo "${BLUE}╚════════════════════════════════════╝${NC}"
    echo ""

    local _found=0
    local _binary_path=""

    # Find binary in PATH
    if command -v "${BINARY_NAME}" >/dev/null 2>&1; then
        _binary_path="$(command -v "${BINARY_NAME}")"
        _found=1
    elif [ -f "${INSTALL_DIR}/${BINARY_NAME}" ]; then
        _binary_path="${INSTALL_DIR}/${BINARY_NAME}"
        _found=1
    fi

    if [ "$_found" -eq 1 ]; then
        echo "${BLUE}→${NC} Removing binary: ${_binary_path}"
        rm -f "${_binary_path}"
        echo "${GREEN}✓${NC} Binary removed"
    else
        echo "${YELLOW}!${NC} Binary not found in PATH or ${INSTALL_DIR}"
    fi

    # Remove config directory
    local _config_dir="${HOME}/.config/retrotls"
    if [ -d "${_config_dir}" ]; then
        echo "${BLUE}→${NC} Removing config directory: ${_config_dir}"
        rm -rf "${_config_dir}"
        echo "${GREEN}✓${NC} Config directory removed"
    fi

    echo ""
    echo "${GREEN}Done!${NC} RetroTLS has been uninstalled."
}

# Parse arguments
case "${1:-}" in
    --uninstall|-u)
        uninstall_retrotls
        ;;
    --help|-h)
        echo "RetroTLS Installer"
        echo ""
        echo "Usage:"
        echo "  Install/Update:   curl -fsSL ... | sh"
        echo "  Uninstall:        curl -fsSL ... | sh -s -- --uninstall"
        echo ""
        echo "Environment variables:"
        echo "  INSTALL_DIR       Installation directory (default: ~/.local/bin)"
        ;;
    *)
        install_retrotls
        ;;
esac
