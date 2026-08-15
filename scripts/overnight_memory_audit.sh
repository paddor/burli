#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SMOKE="${SMOKE:-0}"
CPU_COUNT="${CPU_COUNT:-6}"
MIRI_TEST_THREADS="${MIRI_TEST_THREADS:-1}"
if [ "$SMOKE" = "1" ]; then
  FUZZ_TOTAL_SECONDS="${FUZZ_TOTAL_SECONDS:-30}"
  FUZZ_SLICE_SECONDS="${FUZZ_SLICE_SECONDS:-10}"
  RUN_FULL_MIRI="${RUN_FULL_MIRI:-0}"
else
  FUZZ_TOTAL_SECONDS="${FUZZ_TOTAL_SECONDS:-28800}"
  FUZZ_SLICE_SECONDS="${FUZZ_SLICE_SECONDS:-900}"
  RUN_FULL_MIRI="${RUN_FULL_MIRI:-1}"
fi
FUZZ_JOBS="${FUZZ_JOBS:-$CPU_COUNT}"
RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
LOG_DIR="${LOG_DIR:-$ROOT/tmp/overnight-memory-audit/$RUN_ID}"
STATUS_LOG="$LOG_DIR/status.log"
SUMMARY_LOG="$LOG_DIR/summary.tsv"
MIRI_STRICT_FLAGS="${MIRI_STRICT_FLAGS:--Zmiri-symbolic-alignment-check}"
RUN_FUZZ="${RUN_FUZZ:-1}"

export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=1:halt_on_error=1:abort_on_error=1:symbolize=1}"
export LSAN_OPTIONS="${LSAN_OPTIONS:-print_suppressions=0}"

mkdir -p "$LOG_DIR"
: >"$STATUS_LOG"
printf 'phase\tstatus\texit\tstart\tend\tseconds\tlog\n' >"$SUMMARY_LOG"

stamp() {
  date -Iseconds
}

status() {
  local line
  line="$(stamp) $*"
  printf '%s\n' "$line"
  printf '%s\n' "$line" >>"$STATUS_LOG"
}

run_logged() {
  local phase="$1"
  shift
  local log="$LOG_DIR/$phase.log"
  local start_iso end_iso start_epoch end_epoch elapsed rc outcome

  start_iso="$(stamp)"
  start_epoch="$(date +%s)"
  status "START $phase log=$log"
  {
    printf 'phase: %s\n' "$phase"
    printf 'start: %s\n' "$start_iso"
    printf 'command:'
    printf ' %q' "$@"
    printf '\n\n'
    "$@"
  } >"$log" 2>&1
  rc=$?

  end_iso="$(stamp)"
  end_epoch="$(date +%s)"
  elapsed=$((end_epoch - start_epoch))
  if [ "$rc" -eq 0 ]; then
    outcome="ok"
  else
    outcome="fail"
  fi

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$phase" "$outcome" "$rc" "$start_iso" "$end_iso" "$elapsed" "$log" \
    >>"$SUMMARY_LOG"
  status "DONE $phase status=$outcome exit=$rc seconds=$elapsed"
  return "$rc"
}

failures=0

run_logged cargo_test cargo test --workspace --all-targets || failures=$((failures + 1))
run_logged paranoid cargo test --workspace --all-targets --features paranoid || failures=$((failures + 1))
run_logged miri_unsafe scripts/miri_unsafe_primitives.sh || failures=$((failures + 1))

if [ "$RUN_FULL_MIRI" = "1" ]; then
  run_logged miri_alloc env MIRIFLAGS="$MIRI_STRICT_FLAGS" \
    cargo +nightly miri test -j "$CPU_COUNT" \
      --no-default-features \
      --features alloc \
      -- \
      --test-threads="$MIRI_TEST_THREADS" || failures=$((failures + 1))
fi

if [ "$RUN_FUZZ" = "1" ]; then
  run_logged fuzz env \
    FUZZ_TOTAL_SECONDS="$FUZZ_TOTAL_SECONDS" \
    FUZZ_SLICE_SECONDS="$FUZZ_SLICE_SECONDS" \
    FUZZ_JOBS="$FUZZ_JOBS" \
    RUN_ID="$RUN_ID" \
    LOG_DIR="$LOG_DIR/fuzz" \
    scripts/overnight_fuzz.sh || failures=$((failures + 1))
fi

status "memory audit complete failures=$failures summary=$SUMMARY_LOG"
exit "$failures"
