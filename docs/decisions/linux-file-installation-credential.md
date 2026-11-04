# Linux file-backed installation credential

## Context

Managed startup needs a stable 32-byte installation secret for loopback HTTP
authentication and log redaction. Exclusive startup ownership is enforced by the
StartupOwner lock on the data directory, not by hiding the secret bytes.
macOS stores the secret in Keychain. Linux has no equivalent OS secret store in
our supported stack.

## Decision

On Linux, persist the secret in `<data>/installation-credential/<account>` as a
regular file with mode exactly `0600` inside a directory with mode exactly
`0700`, both owned by the effective user. The store rejects symlinks, FIFOs,
and any mode other than those exact permissions, so stricter modes such as
`0400` are rejected too. A `.partial` leftover that shares the credential's
inode (from an interrupted publish) is cleaned up; any other hard link to the
credential file is refused.

This reverses the earlier “no file fallback on Linux” rule from `main`: Linux
managed startup **requires** this file store; there is no Keychain or
encrypted-at-rest alternative in the product today.

## Alternatives considered

- **Linux Secret Service (libsecret / GNOME Keyring):** Would align with desktop
  session secrets, but adds session-bus coupling, inconsistent headless behaviour,
  and packaging variance across distributions. Deferred until we have a supported
  story for non-interactive managed startup without a logged-in session.

## Consequences

- The key is **not** encrypted at rest; protection is exact `0700`/`0600`
  permissions and same-user access only.
- Home-directory and `XDG_DATA_HOME` backups include the credential file.
- Operators must treat backup media like any other secret-bearing data.
- macOS behaviour is unchanged (Keychain only).
