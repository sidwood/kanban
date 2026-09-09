# Personal macOS packaging

<!-- cspell:words codesign hfsplus koly libisofs mkisofs mountpoint nobrowse notarization rustc UDIF UDZO unnotarized xattr xcrun xorriso -->

## Build and install

Use macOS with Xcode Command Line Tools, Python 3.11+, the repository's Rust,
Node and pnpm toolchains, and `just`. Install the extra **build-only** image
writer with `brew install xorriso`. It is not copied into the application.
The native fixture was exercised with xorriso 1.5.8.pl02 / libisofs 1.5.8.

From a clean checkout:

```sh
just package
```

The output directory defaults to `target/package` and must be absent or empty.
Use a fresh destination to keep multiple runs without overwriting evidence:

```sh
just package --output /tmp/kanban-preview-first
just package --output /tmp/kanban-preview-second
```

Outputs:

- `Kanban.app`: the installable application directory, with normalized modes
  and modification times.
- `Kanban.app.tar`: a byte-comparable serialization of the complete signed
  application, including all code signatures, resources, paths and link
  targets; archive owners and timestamps are fixed.
- `Kanban.dmg`: a compressed, read-only, natively mountable HFS+ hybrid image
  containing `Kanban.app` and an `/Applications` symlink.
- `packaging-report.json`: source identity, dirty-state flag, exact artifact
  SHA-256 values, native target and toolchain versions.
- `build-inputs.json`: the exact source file hashes and modes, identity,
  content-derived compiler root and fresh-target declaration outside the app.

Open the DMG and copy the app to Applications. Herdr is an **external
prerequisite**, not a bundled binary. This command never publishes anything.

These are **ad-hoc signed, unnotarized personal previews**, not Apple-trusted
releases. Gatekeeper may block downloaded previews. No Apple Developer
membership, signing secret, notarization or updater publication is attempted.
The app is signed with identity `-`, no signing timestamp, and verified with
`codesign --verify --deep --strict`. Packaging removes inherited Apple signing
and Tauri signing-secret variables before invoking the bundler.

`--allow-dirty` is only for local tooling development. The report marks such
builds `source_dirty: true`; their HEAD identity is not proof that the working
tree equals that revision. They are **not clean-checkout acceptance evidence**.

## Release hook and installed paths

The ordinary `tauri.conf.json` keeps bundling inactive and declares no generated
inputs. `just package` activates `tauri.release.conf.json` with Tauri's
`--config` option and builds only the `app` bundle, not Tauri's stock DMG.
The release overlay inherits the baseline CSP, window and capability posture
without replacing any of them.

Tauri runs `python3 ../../scripts/package.py prepare` from `apps/desktop`.
The hook refuses to run without the wrapper's matching source environment. It:

1. Builds `kanban-service` and `kanban-mcp` with Cargo `--locked --release`
   for the native target.
2. Copies those actual executables to target-suffixed `externalBin` inputs.
3. Verifies the release service's real `--version` JSON against the build
   identity before emitting `resources/build-identity.json`.
4. Generates a deterministic 512px ICNS from the tracked 32px PNG using native
   upscaling and a PNG-backed ICNS container, not replacement artwork.
5. Builds the locked frontend through its existing `build:web` command.

Tauri bundles:

```text
Kanban.app/Contents/
  Info.plist
  MacOS/
    kanban-desktop
    kanban-service
    kanban-mcp
  Resources/
    icon.icns
    resources/build-identity.json
```

The resource identity has exactly `version`, `source_revision` and
`source_epoch`; proof metadata stays in the report outside the app. Generated
sidecars, identity JSON and ICNS are ignored by Git. Herdr, build tools, Pilot,
analytics and crash-reporting components are not added by this tooling.

## Reproducibility mechanism and limits

The wrapper derives the full revision and commit epoch from Git, checks that
workspace, shell and frontend versions agree, and sets
`KANBAN_SOURCE_REVISION` / `SOURCE_DATE_EPOCH` for both Cargo workspaces.
Rust source and Cargo-home paths are remapped, native C/C++ file paths are
remapped, incremental compilation is disabled, and archive dates, locale and
timezone are controlled. Remapping alone is insufficient: Cargo hashes absolute
paths for dependencies outside the current workspace into crate metadata.
The deliberately separate shell workspace consumes the root crates this way,
so a checkout move changes linked desktop code even with identical sources.

The wrapper exports the real Git revision's blobs, verifies their object hashes,
and builds an owned snapshot at
`/private/tmp/kanban-package-inputs/<input-sha256>/source`. The input digest covers
every exported file's bytes and Git mode plus the real revision/version/epoch.
There is no synthetic `.git` directory or invented source revision. Dirty tooling
builds instead export current non-ignored working files and remain labelled dirty.
The release hook verifies the staged manifest, file hashes, modes and identity
against the controlling environment before generating sidecars or frontend assets.

The staging base must be an owned mode-0700 directory with an owned mode-0600
lock. Concurrent packaging is refused; retry after the active build finishes.
An existing input directory is never reused or overwritten, including interrupted
work. Each invocation creates fresh Cargo targets and generated inputs and removes
only its own staging directory on exit. The checkout's existing build products
remain untouched. An interrupted process that bypasses cleanup requires inspection
of the retained directory before a manual, ownership-checked cleanup and retry.

This controls compiler inputs, not cryptographic entropy: Tauri's runtime invoke
key, application authentication, CSP, capabilities and code signatures are unchanged.
No dependency fork, generated-context rewrite, or binary-byte stripping is involved.
See Cargo's [external-path identity issue](https://github.com/rust-lang/cargo/issues/13586).

Use the **same native architecture, Rust/LLVM,
Node/pnpm, macOS SDK, macOS image tools and xorriso versions** for comparison.
Locked dependencies are necessary but do not make different toolchains
byte-identical; consult the recorded versions if comparison fails.

Stock `hdiutil create` introduces filesystem creation dates and random volume
identity. Instead, xorriso writes the HFS+ hybrid with fixed file dates, volume
dates, ownership and serial number. Startup configuration is disabled with
`-no_rc` as the first xorriso argument. Two narrowly scoped metadata steps are
necessary:

- libisofs 1.5.8 emits `????/????` Finder type/creator defaults on regular
  files. Strict macOS code verification rejects these as Finder detritus.
  The packager parses the Apple partition map and HFS+ catalog and clears only
  those exact generated fields before compression. Unknown metadata and
  fragmented catalogs are rejected; application file bytes are never searched
  and replaced. Symlink metadata remains intact.
- `hdiutil convert -format UDZO -tasks 1` adds a random 16-byte UDIF segment
  identifier. In the validated version-4, single-segment trailer it occupies
  bytes 64–79, not 72–87. The packager replaces it with the first 16 bytes of
  the raw image's SHA-256 **before any image signing**. The native data
  checksums remain valid. No other footer or compressed data is changed.

References: [HFS+ catalog format](https://developer.apple.com/library/archive/technotes/tn/tn1150.html),
[UDIF trailer layout](https://www.mothersruin.com/software/Archaeology/reverse/udif.html),
[xorriso reproducibility options](https://www.gnu.org/software/xorriso/man_1_xorrisofs.html),
[libisofs HFS+ writer](https://dev.lovelyhq.com/libburnia/libisofs/src/branch/master/libisofs/hfsplus.c).

Every generated DMG passes `hdiutil verify`, read-only native attach, complete
mounted-file hash/mode/link comparison, strict deep signature verification and
detach before success is reported. Native fixture tests additionally execute
the mounted fixture. This is **full DMG byte equality**, not a claim that
unequal images become equal when their contents are compared canonically.
Do not suppress an image-byte mismatch or signature failure.

Do not remove Mach-O UUID load commands for reproducibility: macOS 26's loader
requires them. Host filesystem birth/access times of a directory are not a
serialized artifact; compare the entire `.app.tar` for application bytes and
fixed packaging metadata, and the **entire `.dmg`** for disk-image bytes.

## Verification

```sh
just check-packaging          # fast, portable, includes real small Cargo regression
just check-packaging-native   # actual small fixture; requires macOS/xorriso
just check-packaging-build    # actual app build and two-root byte regression; expensive
```

The native fixture creates independent source trees in different paths,
changes source dates and insertion order, supplies conflicting xorriso user
configuration on the second run, compares complete DMG bytes, mounts both
images and checks their signatures, resource bytes, link and executable.
The fixture is explicitly a small compiled test program, not a substituted
service or fabricated product-smoke result.

Final acceptance still requires **two independently clean checkouts**, without
copied Cargo targets or generated sidecars, at the same commit and toolchain:

```sh
# Run once in each clean checkout, with distinct empty output destinations.
just package --output /tmp/kanban-clean-first
just package --output /tmp/kanban-clean-second
cmp /tmp/kanban-clean-first/Kanban.app.tar /tmp/kanban-clean-second/Kanban.app.tar
cmp /tmp/kanban-clean-first/Kanban.dmg /tmp/kanban-clean-second/Kanban.dmg
shasum -a 256 /tmp/kanban-clean-{first,second}/Kanban.app.tar
shasum -a 256 /tmp/kanban-clean-{first,second}/Kanban.dmg
```

Those two `just package` lines are run in their respective checkouts, not both
in the same one. `check-packaging-build` also creates independent source clones,
applies the current working changes without inventing commits, checks that no
targets or generated sidecars were copied, builds both actual applications, and
compares complete archive and DMG bytes. It retains honest dirty-state reports;
it is not a replacement for final clean-checkout acceptance. Set
`KANBAN_PACKAGING_TEST_ROOT` to a new absolute directory to retain those clones,
preflight receipts, logs and artifacts rather than deleting temporary test output.
The small Cargo test reproduces the separate-workspace external-path boundary,
but is not application proof. The installed shell smoke is separate: run the
packaged shell's `--package-smoke` with a **nonexistent**
new absolute data path, the real user's HOME (required by macOS
Security.framework's native Keychain lookup), minimal `/usr/bin:/bin` PATH,
an isolated temporary working directory and no development variables. Its
runtime acceptance covers service discovery, identity, Herdr diagnostics and
verified clean shutdown. The disposable custom data directory uses a scoped
Keychain account; clean up and verify deletion of only that test account, never
the default installation's account. A fake HOME is not a valid native Keychain
smoke environment.

The repeatable installed-artifact driver performs these steps and writes a
fail-closed receipt:

```sh
just package-smoke --dmg target/package/Kanban.dmg --report temp/installed-smoke.json
```

It mounts the actual image read-only, copies its app into a temporary
Applications directory, detaches the image, verifies the copy's signature,
and runs the copied shell and service from an isolated working directory.
It checks the native account was absent before startup, and deletes and
verifies removal of only that owned temporary account after the probe.
It never requests or prints a stored credential. A receipt reports success
only after the process, socket, credential and temporary install are cleaned up.

## Tauri Pilot evaluation

Pilot was desk-evaluated from its [upstream README](https://github.com/mpiton/tauri-pilot),
not adopted for this packaging ticket. It starts a Unix socket / named pipe
and injects a JavaScript bridge. Its documentation guards registration with
`#[cfg(debug_assertions)]`, requires a `pilot:default` capability, and shows an
unpinned Git dependency in the quickstart. **No Pilot runtime trial was
performed.** No plugin, capability, analytics or crash reporter was added.

Native/Rust and typed-IPC tests remain the chosen verification path. A future
Pilot trial must use isolated debug-only configuration and a pinned source
revision, with no release capability changes. See also
[Tauri's debug guidance](https://tauri.app/develop/debug/) for debug versus
production developer-tools behavior.
