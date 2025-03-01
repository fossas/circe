#!/usr/bin/env bash
set -euo pipefail

# circe installer script
# 
# Usage:
#   curl -sSfL https://raw.githubusercontent.com/fossas/circe/main/install.sh | bash
#   curl -sSfL https://raw.githubusercontent.com/fossas/circe/main/install.sh | bash -s -- -b /usr/local/bin
#   curl -sSfL https://raw.githubusercontent.com/fossas/circe/main/install.sh | bash -s -- -v v0.5.0
#
# Note: For versions v0.4.0 and earlier, please use the installer attached to the specific
# GitHub release: https://github.com/fossas/circe/releases/tag/vX.Y.Z
#
# Options:
#   -v, --version    Specify a version (default: latest)
#   -b, --bin-dir    Specify the installation directory (default: $HOME/.local/bin)
#   -t, --tmp-dir    Specify the temporary directory (default: system temp directory)
#   -h, --help       Show help message

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

# Fail with an error message
fail() {
  echo -e "${RED}Error: $1${NC}" >&2
  exit 1
}

# Print an informational message
info() {
  echo -e "${GREEN}$1${NC}" >&2
}

# Print a warning message
warn() {
  echo -e "${YELLOW}Warning: $1${NC}" >&2
}

# Detect the operating system and architecture
detect_platform() {
  local kernel
  local machine
  local os
  local arch

  kernel=$(uname -s)
  machine=$(uname -m)

  case "$kernel" in
    Linux)
      os="unknown-linux"
      ;;
    Darwin)
      os="apple-darwin"
      ;;
    MINGW* | MSYS* | CYGWIN*)
      fail "Windows is not supported by this installer. Please download the Windows binary from the GitHub releases page."
      ;;
    *)
      fail "Unsupported operating system: $kernel"
      ;;
  esac

  case "$machine" in
    x86_64 | amd64)
      arch="x86_64"
      ;;
    arm64 | aarch64)
      arch="aarch64"
      ;;
    *)
      fail "Unsupported architecture: $machine"
      ;;
  esac

  # Check for musl instead of glibc on Linux
  if [[ "$os" == "unknown-linux" ]]; then
    if [[ -e /etc/alpine-release ]] || ldd /bin/sh | grep -q musl; then
      os="$os-musl"
    else
      os="$os-gnu"
    fi
  fi

  echo "${arch}-${os}"
}

# Parse command line arguments
parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -v|--version)
        VERSION="$2"
        # Check if it's an older version and show a warning
        if [[ "$2" == "v0.4.0" || "$2" < "v0.4.0" ]]; then
          warn "You're installing version $2, which may not be compatible with this installer."
          warn "For versions v0.4.0 and earlier, please use the installer attached to the GitHub release:"
          warn "https://github.com/fossas/circe/releases/tag/$2"
          warn "Continuing anyway, but installation may fail."
          
          # Checking if we're in a pipe or tty for interactive prompts
          if [[ -t 0 ]]; then
            echo ""
            echo "Do you want to continue anyway? [y/N]"
            read -r response
            if [[ ! "$response" =~ ^[yY]$ ]]; then
              echo "Installation cancelled"
              exit 1
            fi
          else
            # When piped to bash, we'll continue with a warning
            echo ""
            warn "Running non-interactively, continuing with installation."
            warn "Press Ctrl+C now to cancel if needed."
            sleep 3
          fi
        fi
        shift 2
        ;;
      -b|--bin-dir)
        BIN_DIR="$2"
        shift 2
        ;;
      -t|--tmp-dir)
        TMP_DIR="$2"
        shift 2
        ;;
      -h|--help)
        echo "circe installer"
        echo
        echo "Usage: curl -sSfL https://raw.githubusercontent.com/fossas/circe/main/install.sh | bash [args]"
        echo
        echo "Options:"
        echo "  -v, --version    Specify a version (default: latest)"
        echo "  -b, --bin-dir    Specify the installation directory (default: \$HOME/.local/bin)"
        echo "  -t, --tmp-dir    Specify the temporary directory (default: system temp directory)"
        echo "  -h, --help       Show this help message"
        echo
        echo "Note: For versions v0.4.0 and earlier, please use the installer attached to the GitHub release."
        exit 0
        ;;
      *)
        fail "Unknown option: $1"
        ;;
    esac
  done
}

# Get the latest version number from GitHub
get_latest_version() {
  local url="https://api.github.com/repos/fossas/circe/releases/latest"
  local version
  
  if ! version=$(curl -sSfL "$url" | grep -o '"tag_name": "v[^"]*"' | cut -d'"' -f4); then
    fail "Failed to get latest version from GitHub"
  fi
  
  echo "$version"
}

# Download a file
download() {
  local url="$1"
  local dest="$2"
  
  info "Downloading $url to $dest"
  
  if ! curl -sSfL "$url" -o "$dest"; then
    fail "Failed to download from $url"
  fi
}

# Install the binary
install_binary() {
  local platform="$1"
  local version="$2"
  local bin_dir="$3"
  local tmp_dir="$4"
  local download_url
  local checksums_url
  local archive_name
  local binary_name="circe"
  local release_prefix="https://github.com/fossas/circe/releases/download/${version}"

  # Determine archive name based on platform
  if [[ "$platform" == *"-windows-"* ]]; then
    archive_name="circe-${platform}.zip"
    binary_name="circe.exe"
  else
    archive_name="circe-${platform}.tar.gz"
  fi

  # Construct download URLs
  download_url="${release_prefix}/${archive_name}"
  checksums_url="${release_prefix}/checksums.txt"

  # Create temporary directory
  local workdir="$tmp_dir/circe-install-$$"
  mkdir -p "$workdir"
  cd "$workdir"

  # Download archive and checksums
  download "$download_url" "$archive_name"
  download "$checksums_url" "checksums.txt"

  # Verify checksum
  info "Verifying checksum"
  local expected_checksum
  expected_checksum=$(grep "$archive_name" checksums.txt | awk '{print $1}')
  if [[ -z "$expected_checksum" ]]; then
    fail "Couldn't find checksum for $archive_name"
  fi

  local actual_checksum
  if command -v sha256sum > /dev/null; then
    actual_checksum=$(sha256sum "$archive_name" | awk '{print $1}')
  elif command -v shasum > /dev/null; then
    actual_checksum=$(shasum -a 256 "$archive_name" | awk '{print $1}')
  else
    fail "Neither sha256sum nor shasum found, cannot verify download"
  fi

  if [[ "$expected_checksum" != "$actual_checksum" ]]; then
    fail "Checksum verification failed! Expected: $expected_checksum, got: $actual_checksum"
  fi

  # Extract archive
  if [[ "$archive_name" == *.tar.gz ]]; then
    tar -xzf "$archive_name"
  elif [[ "$archive_name" == *.zip ]]; then
    if command -v unzip > /dev/null; then
      unzip -o "$archive_name"
    else
      fail "The unzip command is required to install circe on Windows"
    fi
  fi

  # Create bin directory if it doesn't exist
  mkdir -p "$bin_dir"

  # Find and copy the binary
  local extracted_binary
  extracted_binary=$(find . -name "$binary_name" -type f | head -n 1)
  if [[ -z "$extracted_binary" ]]; then
    fail "Could not find $binary_name in the extracted archive"
  fi

  cp "$extracted_binary" "$bin_dir/circe"
  chmod +x "$bin_dir/circe"

  # Clean up
  cd - > /dev/null
  rm -rf "$workdir"

  info "Installed circe to $bin_dir/circe"

  # Check if bin_dir is in PATH
  if [[ ":$PATH:" != *":$bin_dir:"* ]]; then
    warn "$bin_dir is not in your PATH. You may need to add it."
    if [[ "$SHELL" == */zsh ]]; then
      echo "To add it, you can run: echo 'export PATH=\"\$PATH:$bin_dir\"' >> ~/.zshrc"
    elif [[ "$SHELL" == */bash ]]; then
      echo "To add it, you can run: echo 'export PATH=\"\$PATH:$bin_dir\"' >> ~/.bashrc"
    elif [[ "$SHELL" == */fish ]]; then
      echo "To add it, you can run: fish -c \"fish_add_path $bin_dir\""
    else
      echo "To add it, add the following to your shell's init file: export PATH=\"\$PATH:$bin_dir\""
    fi
  fi
}

# Main function
main() {
  # Set defaults
  local VERSION=""
  local BIN_DIR="$HOME/.local/bin"
  local TMP_DIR="${TMPDIR:-/tmp}"

  # Parse command line arguments
  parse_args "$@"

  # Detect platform
  local PLATFORM
  PLATFORM=$(detect_platform)
  info "Detected platform: $PLATFORM"

  # If version not specified, get latest
  if [[ -z "$VERSION" ]]; then
    VERSION=$(get_latest_version)
    info "Using latest version: $VERSION"
  fi

  # Install binary
  install_binary "$PLATFORM" "$VERSION" "$BIN_DIR" "$TMP_DIR"

  info "Installation complete! Run 'circe --help' to get started."
}

# Run main function
main "$@"