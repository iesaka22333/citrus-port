#!/usr/bin/env bash
# CitrusPort deployment packaging script for Linux/Unix shells.
# Builds (optionally) and packages platform archives into dist/.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

PLATFORM="native"
PROFILE="release"
TARGET_OVERRIDE=""
OUT_DIR="$ROOT_DIR/dist"
SKIP_BUILD=0
OFFLINE=0
DO_CLEAN=0
HOST_OS="$(uname -s)"
HOST_ARCH="$(uname -m)"

usage() {
    cat <<'EOF'
CitrusPort deployment packaging script (Linux/Unix)

Usage:
  scripts/deploy.sh [options]

Options:
  --platform <native|linux|windows|all>  Target platform (default: native)
  --profile <debug|release|both>         Build profile (default: release)
  --target <triple>                      Override the Rust target triple.
  --out <directory>                      Output directory (default: dist)
  --skip-build                           Package existing artifacts only.
  --offline                              Pass --offline to cargo when building.
  --clean                                Run cargo clean before building.
  -h, --help                             Show this help.

Outputs:
  Linux:   dist/citrus-port-<version>-<target>-<profile>.tar.gz
  Windows: dist/citrus-port-<version>-<target>-<profile>.zip
  Also writes dist/SHA256SUMS.

Examples:
  scripts/deploy.sh --platform linux --profile release --offline
  scripts/deploy.sh --platform windows --profile release --target x86_64-pc-windows-gnu
  scripts/deploy.sh --platform both --profile debug --skip-build
EOF
}

die() {
    echo "error: $*" >&2
    exit 2
}

require_value() {
    if [[ $# -lt 2 || -z "${2-}" ]]; then
        die "$1 requires a value"
    fi
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --platform)
            require_value "$1" "${2-}"
            PLATFORM="$2"
            shift 2
            ;;
        --platform=*)
            PLATFORM="${1#*=}"
            shift
            ;;
        --profile|--mode)
            require_value "$1" "${2-}"
            PROFILE="$2"
            shift 2
            ;;
        --profile=*|--mode=*)
            PROFILE="${1#*=}"
            shift
            ;;
        --target)
            require_value "$1" "${2-}"
            TARGET_OVERRIDE="$2"
            shift 2
            ;;
        --target=*)
            TARGET_OVERRIDE="${1#*=}"
            shift
            ;;
        --out)
            require_value "$1" "${2-}"
            OUT_DIR="$2"
            shift 2
            ;;
        --out=*)
            OUT_DIR="${1#*=}"
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --offline)
            OFFLINE=1
            shift
            ;;
        --clean)
            DO_CLEAN=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

case "$PLATFORM" in
    native|linux|windows|all) ;;
    *) die "--platform must be one of: native, linux, windows, all" ;;
esac

case "$PROFILE" in
    debug|release|both) ;;
    *) die "--profile must be one of: debug, release, both" ;;
esac

if [[ -n "$TARGET_OVERRIDE" && "$PLATFORM" == "all" ]]; then
    die "--target cannot be combined with --platform all"
fi

if [[ "$OUT_DIR" != /* ]]; then
    OUT_DIR="$ROOT_DIR/$OUT_DIR"
fi

host_is_windows() {
    [[ "$HOST_OS" == MINGW* || "$HOST_OS" == MSYS* || "$HOST_OS" == CYGWIN* ]]
}

host_triple() {
    rustc -vV 2>/dev/null | awk '/^host:/ { host = $2 } END { print host }'
}

target_label() {
    local target="$1"
    if [[ -n "$target" ]]; then
        echo "$target"
    else
        host_triple
    fi
}

linux_target() {
    if [[ "$HOST_OS" == "Linux" ]]; then
        echo ""
    else
        echo "x86_64-unknown-linux-gnu"
    fi
}

windows_target() {
    if host_is_windows; then
        echo ""
    else
        echo "x86_64-pc-windows-gnu"
    fi
}

resolve_specs() {
    if [[ -n "$TARGET_OVERRIDE" ]]; then
        printf '%s|%s\n' "$PLATFORM" "$TARGET_OVERRIDE"
        return
    fi
    case "$PLATFORM" in
        native)
            printf 'native|\n'
            ;;
        linux)
            printf 'linux|%s\n' "$(linux_target)"
            ;;
        windows)
            printf 'windows|%s\n' "$(windows_target)"
            ;;
        all)
            printf 'linux|%s\n' "$(linux_target)"
            printf 'windows|%s\n' "$(windows_target)"
            ;;
    esac
}

archive_kind() {
    local platform_name="$1"
    local target="$2"
    if [[ -n "$target" ]]; then
        if [[ "$target" == *windows* ]]; then
            echo "zip"
            return
        fi
        if [[ "$target" == *linux* ]]; then
            echo "tar.gz"
            return
        fi
    fi
    case "$platform_name" in
        windows)
            echo "zip"
            ;;
        linux)
            echo "tar.gz"
            ;;
        native)
            if host_is_windows; then
                echo "zip"
            else
                echo "tar.gz"
            fi
            ;;
        *)
            die "unknown platform '$platform_name'"
            ;;
    esac
}

create_zip() {
    local stage="$1"
    local archive="$2"
    local package_name="$3"

    if command -v zip >/dev/null 2>&1; then
        (
            cd "$stage"
            zip -qr "$archive" "$package_name"
        )
    elif command -v python3 >/dev/null 2>&1; then
        python3 - "$stage" "$archive" "$package_name" <<'PY'
import os
import sys
import zipfile

stage, archive, package_name = sys.argv[1], sys.argv[2], sys.argv[3]
root = os.path.join(stage, package_name)
with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zf:
    for dirpath, _dirnames, filenames in os.walk(root):
        for filename in filenames:
            path = os.path.join(dirpath, filename)
            zf.write(path, os.path.relpath(path, stage))
PY
    else
        die "packaging a .zip requires either 'zip' or 'python3'"
    fi
}

sha256_of() {
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{ print $1 }'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{ print $1 }'
    else
        die "neither sha256sum nor shasum is available"
    fi
}

version="$(awk -F'"' '/^version[[:space:]]*=/{ print $2; exit }' Cargo.toml)"
if [[ -z "$version" ]]; then
    version="0.0.0"
fi

profiles=()
case "$PROFILE" in
    debug) profiles=(debug) ;;
    release) profiles=(release) ;;
    both) profiles=(debug release) ;;
esac

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    build_args=(--platform "$PLATFORM" --profile "$PROFILE")
    if [[ -n "$TARGET_OVERRIDE" ]]; then
        build_args+=(--target "$TARGET_OVERRIDE")
    fi
    if [[ "$OFFLINE" -eq 1 ]]; then
        build_args+=(--offline)
    fi
    if [[ "$DO_CLEAN" -eq 1 ]]; then
        build_args+=(--clean)
    fi
    "$SCRIPT_DIR/build.sh" "${build_args[@]}"
fi

mkdir -p "$OUT_DIR"
: >"$OUT_DIR/SHA256SUMS"

while IFS= read -r spec; do
    platform_name="${spec%%|*}"
    target="${spec#*|}"
    kind="$(archive_kind "$platform_name" "$target")"
    label="$(target_label "$target")"

    for profile_name in "${profiles[@]}"; do
        if [[ "$kind" == "zip" ]]; then
            binary_name="citrus-port.exe"
        else
            binary_name="citrus-port"
        fi

        binary_dir="$ROOT_DIR/target"
        if [[ -n "$target" ]]; then
            binary_dir="$binary_dir/$target"
        fi
        binary_dir="$binary_dir/$profile_name"
        binary_path="$binary_dir/$binary_name"

        if [[ ! -f "$binary_path" ]]; then
            echo "error: expected binary not found: $binary_path" >&2
            echo "       Build it first or omit --skip-build." >&2
            exit 1
        fi

        package_name="citrus-port-$version-$label-$profile_name"
        stage="$(mktemp -d)"
        package_dir="$stage/$package_name"
        mkdir -p "$package_dir"
        cp "$binary_path" "$package_dir/"
        cp README.md LICENSE "$package_dir/"

        if [[ "$kind" == "zip" ]]; then
            archive="$OUT_DIR/$package_name.zip"
            create_zip "$stage" "$archive" "$package_name"
        else
            archive="$OUT_DIR/$package_name.tar.gz"
            tar -czf "$archive" -C "$stage" "$package_name"
        fi

        checksum="$(sha256_of "$archive")"
        printf '%s  %s\n' "$checksum" "$(basename -- "$archive")" >>"$OUT_DIR/SHA256SUMS"
        echo "==> Created $archive"
        rm -rf "$stage"
    done
done < <(resolve_specs)

echo "==> Checksums written to $OUT_DIR/SHA256SUMS"
