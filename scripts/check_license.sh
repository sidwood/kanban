#!/usr/bin/env bash
# Focused license gate: canonical root LICENSE, MIT metadata in Cargo and npm
# manifests, README link, and preserved private/publish restrictions.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

fail() {
  printf 'check-license: %s\n' "$*" >&2
  exit 1
}

canonical_body='Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.'

check_root_license() {
  [[ -f LICENSE ]] || fail "root LICENSE is missing"

  local license_text
  license_text="$(<LICENSE)"

  [[ "$license_text" == *$'MIT License\n\nCopyright (c) 2026 Sid Wood\n\n'"$canonical_body" ]] \
    || fail "root LICENSE is not the canonical MIT text with Copyright (c) 2026 Sid Wood"
}

check_cargo_metadata() {
  command -v cargo >/dev/null 2>&1 || fail "cargo is required for license metadata checks"

  local metadata missing
  metadata="$(cargo metadata --no-deps --format-version 1)"
  missing="$(
    METADATA="$metadata" python3 - <<'PY'
import json, os, sys

metadata = json.loads(os.environ["METADATA"])
expected = {
    "kanban-domain",
    "kanban-dto",
    "kanban-app",
    "kanban-storage",
    "kanban-transport",
    "kanban-herdr",
    "kanban-mcp",
    "kanban-service",
    "kanban-fake-core",
}
seen = set()
problems = []
for package in metadata["packages"]:
    name = package["name"]
    if name not in expected:
        continue
    seen.add(name)
    if package.get("license") != "MIT":
        problems.append(f"{name}: expected license MIT, got {package.get('license')!r}")
    publish = package.get("publish")
    if publish not in (False, []):
        problems.append(f"{name}: expected publish restricted, got {publish!r}")
for name in sorted(expected - seen):
    problems.append(f"{name}: missing from cargo metadata")
print("\n".join(problems))
PY
  )"
  [[ -z "$missing" ]] || fail "$missing"

  local desktop_license desktop_publish
  desktop_license="$(
    cargo metadata --no-deps --format-version 1 --manifest-path apps/desktop/src-tauri/Cargo.toml \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0].get("license"))'
  )"
  desktop_publish="$(
    cargo metadata --no-deps --format-version 1 --manifest-path apps/desktop/src-tauri/Cargo.toml \
      | python3 -c 'import json,sys; publish=json.load(sys.stdin)["packages"][0].get("publish"); print("restricted" if publish in (False, []) else "open")'
  )"
  [[ "$desktop_license" == "MIT" ]] \
    || fail "kanban-desktop: expected license MIT, got $desktop_license"
  [[ "$desktop_publish" == "restricted" ]] \
    || fail "kanban-desktop: expected publish restricted, got ${desktop_publish}"
}

check_npm_packages() {
  command -v node >/dev/null 2>&1 || fail "node is required for npm license checks"

  local problems
  problems="$(
    node <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const manifests = [
  "package.json",
  "packages/contracts/package.json",
  "apps/desktop/package.json",
];

const problems = [];
for (const manifestPath of manifests) {
  const raw = fs.readFileSync(path.join(process.cwd(), manifestPath), "utf8");
  const manifest = JSON.parse(raw);
  if (manifest.license !== "MIT") {
    problems.push(`${manifestPath}: expected license MIT, got ${manifest.license ?? "undefined"}`);
  }
  if (manifest.private !== true) {
    problems.push(`${manifestPath}: expected private true, got ${manifest.private ?? "undefined"}`);
  }
}
process.stdout.write(problems.join("\n"));
NODE
  )"
  [[ -z "$problems" ]] || fail "$problems"
}

check_readme_link() {
  [[ -f README.md ]] || fail "root README.md is missing"
  grep -Fq '[MIT License](LICENSE)' README.md \
    || fail "README.md must link to LICENSE with [MIT License](LICENSE)"
}

check_root_license
check_cargo_metadata
check_npm_packages
check_readme_link
