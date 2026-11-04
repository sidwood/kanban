# Weekly progress report: staging environment

This report is posted word for word as an issue comment. Every byte of it
must come back unchanged from `list_comments`, including the headings, the
tables, the code blocks, the checklists, and the final newline.

## Summary

The staging environment is **mostly ready**. Two items remain open, one is
blocked on an owner decision, and the rest landed this week. Nothing here is
a secret; the hosts and numbers are illustrative.

> Note: the database migration ran twice on Tuesday. The second run was a
> no-op, which is the behaviour we want from an idempotent migration.

## Checklist

- [x] Provision the database cluster
- [x] Write the deployment runbook
- [ ] Rotate the staging credentials
- [ ] Decide on the backup retention period
  - [ ] Ask the owner for a retention number
  - [x] Price the storage for 7, 30 and 90 days

## Timeline

| Day       | Work                         | Outcome                  |
|-----------|------------------------------|--------------------------|
| Monday    | Cluster provisioned          | Healthy after 12 minutes |
| Tuesday   | Migration rehearsal          | Two runs, second no-op   |
| Wednesday | Runbook review               | Three fixes merged       |
| Thursday  | Load test at 200 requests/s  | p95 at 180 ms            |
| Friday    | Owner review                 | Retention still open     |

## Commands used

```sh
just check
cargo test --workspace --locked
pnpm -r run test
```

```json
{
  "environment": "staging",
  "replicas": 3,
  "features": ["audit-log", "read-replica"],
  "notes": "quotes \"inside\" strings and a backslash \\ survive"
}
```

    An indented code block, kept as four leading spaces.
	A line that starts with a tab.

## Risks

1. The backup retention decision blocks the production cut-over.
2. Credential rotation needs a maintenance window:
   - the window must be announced 48 hours ahead;
   - rollback must be rehearsed first.
3. Load at 200 requests/s is fine; at 400 requests/s it is not yet measured.

## Characters that must survive

Accents: café, naïve, façade, São Paulo, résumé.
Symbols: © ® ™ € £ ¥ § ¶ • … — – ± × ÷ ≤ ≥ ≠ ∞ → ← ↔ ✓ ✗ ✅ ⚠️ 🚀
Markdown punctuation: *emphasis*, _underscore_, `inline code`, ~~struck~~,
a [link](https://example.com/report?week=45&team=lin#summary), an image
reference ![diagram](https://example.com/diagram.png), and an HTML entity
&amp; that stays as typed.

Escapes: \*not emphasis\*, \_not underscore\_, a literal backtick \`, and a
pipe \| outside a table.

---

## Next week

- Rotate the staging credentials in the Tuesday window.
- Record the owner's retention decision on the issue.
- Re-run the load test at 400 requests/s and attach the results.

Posted by the reporter.
