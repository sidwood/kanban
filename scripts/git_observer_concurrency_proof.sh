#!/usr/bin/env bash
# Prove kanban-service git observer gates stay deterministic under the
# reviewer's stress shape: eight parallel groups of five runs plus a
# loaded two-hundred-iteration loop, all beside an unrelated Cargo
# process that keeps re-acquiring the shared build lock (KAN-T31
# bounce 2; hermetic per KAN-T96).
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

groups="${GIT_OBSERVER_STRESS_GROUPS:-8}"
width="${GIT_OBSERVER_STRESS_WIDTH:-5}"
loaded="${GIT_OBSERVER_STRESS_LOADED:-200}"

work="$(mktemp -d "${TMPDIR:-/tmp}/kanban-git-observer-proof.XXXXXX")"
bystander_pid=""

cleanup() {
  if [[ -n "$bystander_pid" ]] && kill -0 "$bystander_pid" 2>/dev/null; then
    kill "$bystander_pid" 2>/dev/null || true
    wait "$bystander_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# The bystander stands in for the reviewer's overlapping Cargo run.
# No test starts Cargo of its own, so the gate only ever queues
# briefly behind this build instead of deadlocking on it.
(
  while :; do
    cargo build --quiet --package kanban-dto || exit 1
  done
) 2>"$work/bystander.log" &
bystander_pid=$!

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

if ! kill -0 "$bystander_pid" 2>/dev/null; then
  # The bystander died before the proof finished, so the runs after
  # its death were no longer beside an unrelated Cargo process.
  echo "git-observer-concurrency-proof: bystander build failed:" >&2
  cat "$work/bystander.log" >&2 || true
  exit 1
fi
kill "$bystander_pid" 2>/dev/null || true
wait "$bystander_pid" 2>/dev/null || true

echo "git-observer-concurrency-proof: all stress shapes passed"
