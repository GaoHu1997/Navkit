#!/usr/bin/env bash
set -eu
export RUST_MIN_STACK=33554432
cargo check 2> target/sql-history-check.stderr.log
for filter in sql_history::tests manual_transaction::tests execution_tests mutation::tests controlled_execution::tests transfer::transfer_streaming_tests; do
  printf "\n=== %s ===\n" "$filter"
  cargo test --lib "$filter" -- --test-threads=1 2>> target/sql-history-regressions.stderr.log
done
