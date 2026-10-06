#!/bin/sh
# Repository validation entry point for the Rust harness and the TypeScript
# runtime (provider bridge + edit backend).
#
# Every stage runs even if an earlier one failed, and each result is reported
# as one of:
#   PASS  the check ran and succeeded
#   FAIL  the check ran and found a code problem
#   ENV   the check could not run: a tool or dependency is missing or could
#         not be installed (environment problem, not a code verdict)
#   SKIP  the check was not requested or depends on a stage that did not pass
#
# Exit status: 0 all requested stages passed, 1 at least one FAIL, 2 no FAIL
# but at least one ENV.
#
# Usage: Scripts/check.sh [--install] [--no-clippy] [--no-web] [--only STAGES]
#   --install    run `npm ci` for missing JavaScript dependencies (needs network)
#   --no-clippy  skip `cargo clippy` (useful while unrelated lints are pending)
#   --no-web     skip the compatibility web client typecheck
#   --only       comma-separated subset of: rust,runtime,web,integration
set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

INSTALL=0
CLIPPY=1
WEB=1
ONLY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --install) INSTALL=1 ;;
    --no-clippy) CLIPPY=0 ;;
    --no-web) WEB=0 ;;
    --only) shift; ONLY="${1:-}" ;;
    -h|--help) sed -n '2,21p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 64 ;;
  esac
  shift
done

LOG_DIR=$(mktemp -d)
trap 'rm -rf "$LOG_DIR"' EXIT INT TERM
SUMMARY="$LOG_DIR/summary"
: > "$SUMMARY"

wanted() {
  [ -z "$ONLY" ] && return 0
  case ",$ONLY," in *",$1,"*) return 0 ;; esac
  return 1
}

record() { # status name detail
  printf '%-5s %-28s %s\n' "$1" "$2" "${3:-}" >> "$SUMMARY"
  printf '[%s] %s %s\n' "$1" "$2" "${3:-}"
}

# run NAME COMMAND...: runs a code check, records PASS/FAIL, keeps the log tail.
run() {
  name=$1; shift
  log="$LOG_DIR/$(echo "$name" | tr ' /' '__').log"
  printf '==> %s: %s\n' "$name" "$*"
  if "$@" > "$log" 2>&1; then
    record PASS "$name"
    return 0
  fi
  # Failure digest first (failing tests, compiler errors), then the tail.
  grep -E '^test .* FAILED$|^error(\[|:)|panicked at|^ *--> ' "$log" | head -n 40
  echo "... last lines of $name:"
  tail -n 15 "$log"
  record FAIL "$name" "(see output above)"
  return 1
}

have() { command -v "$1" > /dev/null 2>&1; }

# ---------------------------------------------------------------- environment
RUST_READY=1
for tool in cargo rustfmt; do
  if ! have "$tool"; then
    RUST_READY=0
    record ENV "tool: $tool" "not found on PATH"
  fi
done
if [ "$RUST_READY" = 1 ] && [ "$CLIPPY" = 1 ] && ! cargo clippy --version > /dev/null 2>&1; then
  record ENV "tool: clippy" "rustup component add clippy"
  CLIPPY=0
fi
NODE_READY=1
for tool in node npm; do
  if ! have "$tool"; then
    NODE_READY=0
    record ENV "tool: $tool" "not found on PATH (Node is a runtime dependency)"
  fi
done

# deps DIR NAME: ensures node_modules for DIR, installing with --install.
deps() {
  dir=$1; name=$2
  if [ "$NODE_READY" != 1 ]; then
    record SKIP "deps: $name" "node/npm unavailable"
    return 1
  fi
  problem=""
  if [ ! -d "$dir/node_modules" ]; then
    problem="missing $dir/node_modules"
  elif [ "$dir/package-lock.json" -nt "$dir/node_modules/.package-lock.json" ]; then
    problem="$dir/node_modules is older than package-lock.json"
  fi
  if [ -z "$problem" ]; then
    record PASS "deps: $name"
    return 0
  fi
  if [ "$INSTALL" != 1 ]; then
    record ENV "deps: $name" "$problem; rerun with --install (npm ci)"
    return 1
  fi
  if npm --prefix "$dir" ci > "$LOG_DIR/npm-$name.log" 2>&1; then
    record PASS "deps: $name" "(installed with npm ci)"
    return 0
  fi
  tail -n 20 "$LOG_DIR/npm-$name.log"
  record ENV "deps: $name" "npm ci failed (network or registry problem?)"
  return 1
}

# --------------------------------------------------------------- TS runtime
RUNTIME_BUILT=0
if wanted runtime; then
  if deps RuntimeSource runtime; then
    if run "runtime: build" npm --prefix RuntimeSource run build; then
      RUNTIME_BUILT=1
      run "runtime: tests" node --test RuntimeSource/test/*.test.mjs
    else
      record SKIP "runtime: tests" "build failed"
    fi
  fi
fi

# --------------------------------------------------------------------- Rust
if wanted rust; then
  if [ "$RUST_READY" = 1 ]; then
    run "rust: fmt" cargo fmt --all -- --check
    if run "rust: check" cargo check --all-targets; then
      # The source-layout guard and the bridge-backed harness tests (fake
      # provider bridge under Harness/agent/testdata) run as part of the suite.
      run "rust: tests" cargo test --all-targets --no-fail-fast
      if [ "$CLIPPY" = 1 ]; then
        run "rust: clippy" cargo clippy --all-targets -- -D warnings
      else
        record SKIP "rust: clippy" "disabled"
      fi
    else
      record SKIP "rust: tests" "cargo check failed"
    fi
  else
    record SKIP "rust" "Rust toolchain unavailable"
  fi
fi

# --------------------------------------------------------------------- web
if wanted web; then
  if [ "$WEB" != 1 ]; then
    record SKIP "web: typecheck" "disabled"
  elif deps web web; then
    run "web: typecheck" npm --prefix web run typecheck
  fi
fi

# ------------------------------------------------------------- integration
if wanted integration; then
  if [ "$RUST_READY" = 1 ]; then
    TMP_YEET=$(mktemp -d)
    integration() {
      YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- doctor > /dev/null &&
        YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- model set openai/test-model > /dev/null &&
        [ "$(YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- model get)" = "openai/test-model" ]
    }
    run "integration: cli config" integration
    rm -rf "$TMP_YEET"
    if [ "$RUNTIME_BUILT" != 1 ] && ! [ -f RuntimeSource/dist/bridge.js ]; then
      record ENV "integration: bridge" "RuntimeSource/dist/bridge.js missing; build the runtime"
    fi
  else
    record SKIP "integration" "Rust toolchain unavailable"
  fi
fi

echo
echo "Summary:"
cat "$SUMMARY"
if grep -q '^FAIL' "$SUMMARY"; then
  echo "Result: code failures found."
  exit 1
fi
if grep -q '^ENV' "$SUMMARY"; then
  echo "Result: no code failures, but some checks could not run (environment)."
  exit 2
fi
echo "Result: all requested checks passed."
