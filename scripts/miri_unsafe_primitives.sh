#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

filters=(
  "burli-core bits::tests::trusted_writer_matches_checked_writer"
  "burli-encode encode::load::tests::trusted_u32_load_matches_safe_little_endian_load"
  "burli-encode encode::load::tests::trusted_u64_load_matches_safe_little_endian_load"
  "burli-decode compressed::tests::non_overlapping_backward_copy_matches_safe_copy"
  "burli-decode compressed::tests::trusted_fast_literal_bulk_copy_matches_push_loop"
)

for entry in "${filters[@]}"; do
  package="${entry%% *}"
  filter="${entry#* }"
  list="$(cargo test -p "$package" "$filter" -- --exact --list)"
  if [[ "$list" != *"$filter: test"* ]]; then
    printf 'no test matched package=%s filter=%s\n' "$package" "$filter" >&2
    exit 2
  fi
  cargo +nightly miri test -p "$package" "$filter" -- --exact
done
