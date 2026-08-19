#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FUZZ_TOTAL_SECONDS="${FUZZ_TOTAL_SECONDS:-28800}"
FUZZ_SLICE_SECONDS="${FUZZ_SLICE_SECONDS:-900}"
FUZZ_JOBS="${FUZZ_JOBS:-12}"
FUZZ_MAX_LEN="${FUZZ_MAX_LEN:-65536}"
FUZZ_TIMEOUT="${FUZZ_TIMEOUT:-30}"
FUZZ_CORPUS_LIMIT="${FUZZ_CORPUS_LIMIT:-1024}"
FUZZ_GRACE_SECONDS="${FUZZ_GRACE_SECONDS:-120}"
FUZZ_SANITIZER="${FUZZ_SANITIZER:-address}"
FUZZ_DETECT_LEAKS="${FUZZ_DETECT_LEAKS:-1}"
RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
LOG_DIR="${LOG_DIR:-$ROOT/tmp/overnight-fuzz/$RUN_ID}"
FUZZ_HOST="$(rustc -vV | awk '/^host:/ { print $2 }')"
FUZZ_BINARY_DIR="$ROOT/fuzz/target/$FUZZ_HOST/release"
STATUS_LOG="$LOG_DIR/status.log"
SUMMARY_LOG="$LOG_DIR/summary.tsv"

if [ "$FUZZ_SANITIZER" = "none" ] && [ "${FUZZ_DETECT_LEAKS:-1}" = "1" ]; then
  FUZZ_DETECT_LEAKS=0
fi

targets=("$@")
if [ "${#targets[@]}" -eq 0 ]; then
  targets=(burli-decode burli-roundtrip burli-fragmented)
fi

mkdir -p "$LOG_DIR/artifacts" "$LOG_DIR/worker-logs"
: >"$STATUS_LOG"
printf 'target\tstatus\texit\tstart\tend\tseconds\tlog\n' >"$SUMMARY_LOG"

stamp() {
  date -Iseconds
}

status() {
  local line
  line="$(stamp) $*"
  printf '%s\n' "$line"
  printf '%s\n' "$line" >>"$STATUS_LOG"
}

run_target() {
  local target="$1"
  local seconds="$2"
  local target_log_dir="$LOG_DIR/worker-logs/$target"
  local artifact_dir="$LOG_DIR/artifacts/$target/"
  local corpus_dir="$LOG_DIR/corpus/$target"
  local source_corpus="$ROOT/fuzz/corpus/$target"
  local fuzz_binary="$FUZZ_BINARY_DIR/$target"
  local log="$LOG_DIR/$target-$(date +%H%M%S).log"
  local start_iso end_iso start_epoch end_epoch elapsed rc outcome

  mkdir -p "$target_log_dir" "$artifact_dir" "$corpus_dir"
  if [ ! -e "$corpus_dir/.staged" ]; then
    if [ -d "$source_corpus" ]; then
      if [ "$FUZZ_CORPUS_LIMIT" -gt 0 ]; then
        find "$source_corpus" -type f -printf '%s\t%p\n' |
          sort -n |
          awk -v limit="$FUZZ_CORPUS_LIMIT" 'NR <= limit { print }' |
          while IFS=$'\t' read -r _ source; do
            ln "$source" "$corpus_dir/$(basename "$source")"
          done
      else
        find "$source_corpus" -type f -exec ln {} "$corpus_dir/" \;
      fi
    fi
    : >"$corpus_dir/.staged"
  fi
  start_iso="$(stamp)"
  start_epoch="$(date +%s)"
  status "START $target seconds=$seconds workers=$FUZZ_JOBS log=$log"

  set +e
  (
    cd "$target_log_dir"
    timeout \
      --foreground \
      --signal=TERM \
      --kill-after="${FUZZ_GRACE_SECONDS}s" \
      "$((seconds + FUZZ_GRACE_SECONDS))s" \
      "$fuzz_binary" \
        -max_total_time="$seconds" \
        -jobs="$FUZZ_JOBS" \
        -workers="$FUZZ_JOBS" \
        -max_len="$FUZZ_MAX_LEN" \
        -timeout="$FUZZ_TIMEOUT" \
        -detect_leaks="$FUZZ_DETECT_LEAKS" \
        -print_final_stats=1 \
        -artifact_prefix="$artifact_dir" \
        "$corpus_dir"
  ) >"$log" 2>&1
  rc=$?
  set -e

  end_iso="$(stamp)"
  end_epoch="$(date +%s)"
  elapsed=$((end_epoch - start_epoch))
  if [ "$rc" -eq 0 ]; then
    outcome="ok"
  else
    outcome="fail"
  fi

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$target" "$outcome" "$rc" "$start_iso" "$end_iso" "$elapsed" "$log" \
    >>"$SUMMARY_LOG"
  status "DONE $target status=$outcome exit=$rc seconds=$elapsed"
  return "$rc"
}

status "build fuzz targets sanitizer=$FUZZ_SANITIZER"
cargo +nightly fuzz build --fuzz-dir "$ROOT/fuzz" --sanitizer "$FUZZ_SANITIZER"

deadline=$(( $(date +%s) + FUZZ_TOTAL_SECONDS ))
status "overnight fuzz start total_seconds=$FUZZ_TOTAL_SECONDS workers=$FUZZ_JOBS"

while [ "$(date +%s)" -lt "$deadline" ]; do
  for target in "${targets[@]}"; do
    now="$(date +%s)"
    if [ "$now" -ge "$deadline" ]; then
      break
    fi
    remaining=$((deadline - now))
    seconds="$FUZZ_SLICE_SECONDS"
    if [ "$remaining" -lt "$seconds" ]; then
      seconds="$remaining"
    fi
    if ! run_target "$target" "$seconds"; then
      exit 1
    fi
  done
done

status "overnight fuzz complete summary=$SUMMARY_LOG"
