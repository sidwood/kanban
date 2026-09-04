#!/usr/bin/env bash
# Prove the repository's own safety gates bite rather than trusting that
# they are configured: a fresh clone must reject a malformed commit
# message and a staged spelling violation after `just bootstrap` alone.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

work="$(mktemp -d "${TMPDIR:-/tmp}/kanban-check-gates.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# Identity for the probe clone's commit attempts; the Operator's own
# Git identity is irrelevant to what is being proved here.
export GIT_AUTHOR_NAME="Gate Probe"
export GIT_AUTHOR_EMAIL="gate-probe@example.invalid"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME"
export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"

fail() {
  printf 'check-gates: %s\n' "$*" >&2
  exit 1
}

# Run a command that must fail, and prove it failed for the stated
# reason rather than by accident.
expect_failure() {
  local marker="$1" subject="$2"
  shift 2
  local output
  if output="$("$@" 2>&1)"; then
    fail "$subject was accepted:"$'\n'"$output"
  fi
  if ! grep -qF -- "$marker" <<<"$output"; then
    fail "$subject failed without \"$marker\":"$'\n'"$output"
  fi
}

# Attempt a commit in the probe clone and require a hook to refuse it
# without leaving a commit behind.
expect_commit_rejected() {
  local clone="$1" marker="$2" message="$3" subject="$4"
  local before after
  before="$(git -C "$clone" rev-parse HEAD)"
  expect_failure "$marker" "$subject" git -C "$clone" commit -m "$message"
  after="$(git -C "$clone" rev-parse HEAD)"
  [[ "$before" == "$after" ]] || fail "$subject still created a commit"
}

prove_clean_clone_enforces_hooks() {
  local clone="$work/clone"
  git clone --quiet "$root" "$clone"
  # The gate must judge the tree that is about to be committed, not the
  # last commit, so overlay the working tree onto the fresh clone.
  rsync --archive --delete \
    --exclude .git --exclude node_modules --exclude target \
    --exclude dist --exclude temp \
    "$root/" "$clone/"

  [[ -z "$(git -C "$clone" config --get core.hooksPath || true)" ]] \
    || fail "a fresh clone already pointed at a hooks directory"

  local clone_just=(just --justfile "$clone/justfile" --working-directory "$clone")

  # Bootstrap must refuse to leave a hook that cannot run.
  chmod -x "$clone/.githooks/commit-msg"
  expect_failure "not executable" "bootstrap with a hook that cannot run" \
    "${clone_just[@]}" bootstrap
  chmod +x "$clone/.githooks/commit-msg"

  # Bootstrap must refuse when the hook runner is absent, so the
  # spelling gate can never be silently skipped at commit time. The
  # curated PATH keeps just and git but hides pre-commit.
  local probe_bin="$work/bin"
  mkdir -p "$probe_bin"
  ln -s "$(command -v just)" "$probe_bin/just"
  ln -s "$(command -v git)" "$probe_bin/git"
  expect_failure "pre-commit runner" "bootstrap without the hook runner" \
    env PATH="$probe_bin:/usr/bin:/bin" "${clone_just[@]}" bootstrap

  "${clone_just[@]}" bootstrap >"$work/bootstrap.log" 2>&1 \
    || fail "just bootstrap failed:"$'\n'"$(cat "$work/bootstrap.log")"

  [[ "$(git -C "$clone" config --get core.hooksPath)" == ".githooks" ]] \
    || fail "just bootstrap left the tracked hooks disabled"

  # Clean prose in the staged file isolates the commit-message refusal.
  printf 'A clean sentence for the commit gate probe.\n' >"$clone/probe.md"
  git -C "$clone" add probe.md
  expect_commit_rejected "$clone" "commit-msg:" \
    "added a subject line that is far too long to be accepted" \
    "a malformed commit message"

  # A valid subject isolates the spelling refusal. The directive below
  # exempts this script from the spelling gate; the probe file it
  # writes carries no such exemption.
  # cspell:ignore mispelt
  printf 'A deliberately mispelt sentence.\n' >"$clone/probe.md"
  git -C "$clone" add probe.md
  expect_commit_rejected "$clone" "Unknown word" \
    "Add a commit gate probe" \
    "a staged spelling violation"
}

prove_clean_clone_enforces_hooks
