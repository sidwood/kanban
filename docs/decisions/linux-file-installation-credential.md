# Linux file-backed installation credential

## Context

Managed startup needs a stable 32-byte installation secret for loopback HTTP
authentication and installation ownership. macOS stores it in Keychain. Linux has
no equivalent OS secret store in our supported stack.

## Decision

On Linux, persist the secret in `<data>/installation-credential/<account>` as a
regular file with mode `0600` inside a mode `0700` directory owned by the
effective user. The store rejects symlinks, hard links, FIFOs, and
group/world-readable paths.

This reverses the earlier “no file fallback on Linux” rule from `main`: Linux
managed startup **requires** this file store; there is no Keychain or
encrypted-at-rest alternative in the product today.

## Consequences

- The key is **not** encrypted at rest; protection is Unix permissions and
  same-user access only.
- Home-directory and `XDG_DATA_HOME` backups include the credential file.
- Operators must treat backup media like any other secret-bearing data.
- macOS behaviour is unchanged (Keychain only).
