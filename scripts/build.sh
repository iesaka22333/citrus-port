#!/usr/bin/env bash
# CitrusPort build and test script for Linux/Unix shells.
# Run with --help for usage.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

PLATFORM="native"
PROFILE="debug"
TARGET_OVERRIDE=""
RUN_TESTS=0
TEST_ONLY=0
INCLUDE_IGNORED=0
OFFLINE=0
DO_CLEAN=0
REPORT="$ROOT_DIR/test-results.txt"
REPORT_INITIALIZED=0
OVERALL_EXIT=0
HOST_OS="$(uname -s)"
HOST_ARCH="$(uname -m)"

usage() {
    cat <<'EOF'
CitrusPort build and test script (Linux/Unix)

Usage:
  scripts/build.sh [options]

Options:
  --platform <native|linux|windows|all>  Target platform (default: native)
  --profile <debug|release|both>         Build profile (default: debug)
                                         --mode is accepted as an alias.
  --target <triple>                      Override the Rust target triple.
                                         Only valid with a single platform.
  --test                                 Run tests for each selected platform/profile.
  --test-only                            Run tests without building first.
  --ignored                              Also run #[ignore] tests (performance samples).
  --offline                              Pass --offline to cargo.
  --report <path>                        Test report path (default: test-results.txt).
  --clean                                Run cargo clean before building/testing.
  -h, --help                             Show this help.

Examples:
  scripts/build.sh --platform linux --profile debug
  scripts/build.sh --platform linux --profile both --test --offline
  scripts/build.sh --platform windows --profile release --target x86_64-pc-windows-gnu
  scripts/build.sh --test-only --platform linux --offline

Windows cross builds from Linux require:
  rustup target add x86_64-pc-windows-gnu
  a MinGW linker such as x86_64-w64-mingw32-gcc
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
        --test)
            RUN_TESTS=1
            shift
            ;;
        --test-only)
            RUN_TESTS=1
            TEST_ONLY=1
            shift
            ;;
        --ignored)
            INCLUDE_IGNORED=1
            shift
            ;;
        --offline)
            OFFLINE=1
            shift
            ;;
        --report)
            require_value "$1" "${2-}"
            REPORT="$2"
            shift 2
            ;;
        --report=*)
            REPORT="${1#*=}"
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

if [[ "$REPORT" != /* ]]; then
    REPORT="$ROOT_DIR/$REPORT"
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

ensure_target() {
    local target="$1"
    if [[ -z "$target" ]]; then
        return 0
    fi
    if command -v rustup >/dev/null 2>&1; then
        local installed
        installed="$(rustup target list --installed 2>/dev/null || true)"
        if ! grep -Fqx -- "$target" <<<"$installed"; then
            echo "error: Rust target '$target' is not installed." >&2
            echo "       Install it with: rustup target add $target" >&2
            exit 1
        fi
    else
        echo "warning: rustup not found; cannot verify target '$target'." >&2
    fi
    if [[ "$target" == *windows* ]] && ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
        echo "warning: x86_64-w64-mingw32-gcc not found; Windows cross-linking may fail." >&2
    fi
}

init_report() {
    if [[ "$REPORT_INITIALIZED" -eq 1 ]]; then
        return 0
    fi
    mkdir -p "$(dirname -- "$REPORT")"
    {
        echo "CitrusPort Test Report"
        echo "======================"
        echo "Generated (UTC): $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        echo "Host OS: $HOST_OS $HOST_ARCH"
        echo "Repository: $ROOT_DIR"
        echo "Cargo: $(cargo --version 2>/dev/null || echo unknown)"
        echo "Rustc: $(rustc --version 2>/dev/null || echo unknown)"
        echo
    } >"$REPORT"
    REPORT_INITIALIZED=1
}

count_field() {
    local file="$1"
    local word="$2"
    {
        grep -oE "[0-9]+ ${word}" "$file" || true
    } | awk '{ sum += $1 } END { print sum + 0 }'
}

write_test_report() {
    local platform_name="$1"
    local target="$2"
    local profile_name="$3"
    local command="$4"
    local exit_code="$5"
    local output_file="$6"

    local passed failed ignored measured status
    passed="$(count_field "$output_file" passed)"
    failed="$(count_field "$output_file" failed)"
    ignored="$(count_field "$output_file" ignored)"
    measured="$(count_field "$output_file" measured)"

    status="PASS"
    if [[ "$exit_code" -ne 0 || "$failed" -gt 0 ]]; then
        status="FAIL"
    fi

    init_report
    {
        echo "------------------------------------------------------------"
        echo "Platform: $platform_name"
        echo "Target: $(target_label "$target")"
        echo "Profile: $profile_name"
        echo "Command: $command"
        echo "Exit code: $exit_code"
        echo "Status: $status"
        echo "Summary: $passed passed, $failed failed, $ignored ignored, $measured measured"
        echo "Test result lines:"
        grep -E 'Running |test result: |error(\[|:)' "$output_file" | sed 's/^/  /' || true
        echo
        echo "Raw output:"
        sed 's/^/  /' "$output_file"
        echo
    } >>"$REPORT"
}

build_once() {
    local target="$1"
    local profile_name="$2"
    local cargo_args=(build --locked)
    if [[ "$profile_name" == "release" ]]; then
        cargo_args+=(--release)
    fi
    if [[ -n "$target" ]]; then
        cargo_args+=(--target "$target")
    fi
    if [[ "$OFFLINE" -eq 1 ]]; then
        cargo_args+=(--offline)
    fi
    echo "==> cargo ${cargo_args[*]}"
    CARGO_TERM_COLOR=never cargo "${cargo_args[@]}"
}

execute_test() {
    local platform_name="$1"
    local target="$2"
    local profile_label="$3"
    shift 3
    local cargo_args=("$@")
    local command="cargo ${cargo_args[*]}"
    local tmp
    tmp="$(mktemp)"
    echo "==> $command"

    set +e
    CARGO_TERM_COLOR=never cargo "${cargo_args[@]}" 2>&1 | tee "$tmp"
    local exit_code=${PIPESTATUS[0]}
    set -e

    write_test_report "$platform_name" "$target" "$profile_label" "$command" "$exit_code" "$tmp"
    rm -f "$tmp"

    if [[ "$exit_code" -ne 0 ]]; then
        OVERALL_EXIT=1
    fi
}

test_once() {
    local platform_name="$1"
    local target="$2"
    local profile_name="$3"
    local cargo_args=(test --locked)

    if [[ "$profile_name" == "release" ]]; then
        cargo_args+=(--release)
    fi
    if [[ -n "$target" ]]; then
        cargo_args+=(--target "$target")
    fi
    if [[ "$OFFLINE" -eq 1 ]]; then
        cargo_args+=(--offline)
    fi

    execute_test "$platform_name" "$target" "$profile_name" "${cargo_args[@]}"
    if [[ "$INCLUDE_IGNORED" -eq 1 ]]; then
        execute_test "$platform_name" "$target" "$profile_name (ignored tests)" \
            "${cargo_args[@]}" -- --ignored --nocapture
    fi
}

profiles=()
case "$PROFILE" in
    debug) profiles=(debug) ;;
    release) profiles=(release) ;;
    both) profiles=(debug release) ;;
esac

if [[ "$DO_CLEAN" -eq 1 ]]; then
    echo "==> cargo clean"
    cargo clean
fi

while IFS= read -r spec; do
    platform_name="${spec%%|*}"
    target="${spec#*|}"
    ensure_target "$target"

    for profile_name in "${profiles[@]}"; do
        if [[ "$TEST_ONLY" -eq 0 ]]; then
            build_once "$target" "$profile_name"
        fi
        if [[ "$RUN_TESTS" -eq 1 ]]; then
            test_once "$platform_name" "$target" "$profile_name"
        fi
    done
done < <(resolve_specs)

exit "$OVERALL_EXIT"
