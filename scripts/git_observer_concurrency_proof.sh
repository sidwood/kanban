#!/usr/bin/env bash
# Prove kanban-service git observer gates stay deterministic under the
# reviewer's stress shape: eight parallel groups of five runs plus a
# loaded two-hundred-iteration loop (KAN-T31 bounce 2).
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

groups="${GIT_OBSERVER_STRESS_GROUPS:-8}"
width="${GIT_OBSERVER_STRESS_WIDTH:-5}"
loaded="${GIT_OBSERVER_STRESS_LOADED:-200}"

run_gate() {
  cargo test -p kanban-service -- --test-threads=8
}

echo "git-observer-concurrency-proof: ${groups} groups of ${width} parallel gate runs"
for group in $(seq 1 "$groups"); do
  pids=()
  for _ in $(seq 1 "$width"); do
    run_gate &
    pids+=("$!")
  done
  for pid in "${pids[@]}"; do
    wait "$pid"
  done
  echo "  group ${group}/${groups} passed"
done

echo "git-observer-concurrency-proof: ${loaded}-iteration loaded loop"
for iteration in $(seq 1 "$loaded"); do
  run_gate > /dev/null
  if (( iteration % 50 == 0 )); then
    echo "  iteration ${iteration}/${loaded} passed"
  fi
done

echo "git-observer-concurrency-proof: all stress shapes passed"
