#!/usr/bin/env bash
# Exercise the CI workflow command matrix from a fresh public
# checkout: clone the repository, overlay the working tree so the tree
# about to be committed is judged, assert no ignored temp/ planning
# directory exists, then run every `run:` command from the workflow
# verbatim and in order. This proves KAN-T102-AC6: the gates have no
# hidden dependency on local planning artifacts.
#
# The workflow's `uses:` steps (checkout, pnpm, Node) only prepare
# tools on GitHub; this script requires those tools on PATH instead
# and runs the commands the matrix is about. It is an on-demand audit
# deliberately kept out of `just check`, which it ends up running.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

fail() {
  printf 'check-ci-matrix: %s\n' "$*" >&2
  exit 1
}

work="$(mktemp -d "${TMPDIR:-/tmp}/kanban-ci-matrix.XXXXXX")"
trap 'rm -rf "$work"' EXIT

for tool in git node pnpm cargo just brew rsync; do
  command -v "$tool" >/dev/null 2>&1 \
    || fail "$tool is required on PATH; see the README continuous-integration section"
done

clone="$work/clone"
git clone --quiet "$root" "$clone"
# A fresh clone judges the last commit, so overlay the working tree
# the way scripts/check_gates.sh does.
rsync --archive --delete \
  --exclude .git --exclude node_modules --exclude target \
  --exclude dist --exclude temp \
  "$root/" "$clone/"

[[ ! -e "$clone/temp" ]] \
  || fail "the fresh checkout unexpectedly contains temp/"

workflow="$clone/.github/workflows/ci.yml"
[[ -f "$workflow" ]] || fail "the fresh checkout has no CI workflow"

commands=()
while IFS= read -r line; do
  commands+=("$line")
done < <(sed -n -E 's/^[[:space:]]*(- )?run:[[:space:]]*//p' "$workflow")
((${#commands[@]} > 0)) || fail "the CI workflow exposes no run commands"

cd "$clone"
for command in "${commands[@]}"; do
  printf 'check-ci-matrix: %s\n' "$command"
  bash -euo pipefail -c "$command"
done

printf 'check-ci-matrix: %d command(s) passed from a fresh checkout without temp/\n' \
  "${#commands[@]}"
