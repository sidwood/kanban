# Repository instructions

## Work

- Production application source belongs in this repository root.
- Project-management artifacts and fleet state belong under ignored `temp/`.
- Work only in the assigned sibling branch clone. The root checkout is the
  landing seed and must remain untouched by workers.
- Read the assigned Ticket and every linked Spec before changing code.
- Deliver the narrow Ticket scope through every affected layer.
- Write a failing test for each behaviour before implementing it.
- Keep domain rules in Rust application and domain crates, not Vue components
  or Tauri commands.
- Keep the WebView least-privileged. Use typed Tauri commands and events.
- Run the Ticket gate before reporting `REVIEW-READY`.

## Commits

- Repository hooks enforce CSpell and the commit-message convention.
- Write an imperative, capitalised subject of at most 50 characters.
- Separate an optional why-focused body with a blank line and wrap it at 72.
- Keep one concern per commit.
- Keep hooks enabled and repair every failure before committing.

## Product boundaries

- Use Tauri 2, Vue 3, Tailwind 4, Rust, SQLite, and pnpm.
- Cargo owns Rust; pnpm owns frontend dependencies; `just` coordinates them.
- Keep Nx, Bun, Electron, and Canvas Kanban outside the product.
- The SmokeFree Surface board is the visual reference; its domain behaviour is
  not inherited.
