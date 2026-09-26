# ExecPlans

Use an ExecPlan for substantial features, cross-crate refactors, migrations, synchronization work,
or changes that alter domain semantics. Store active plans under `docs/exec/`.

An ExecPlan is a living implementation contract. A fresh agent with only the repository checkout
must be able to execute it without relying on hidden conversation context.

Every ExecPlan must contain:

- **Purpose / observable outcome** — what becomes possible and how to demonstrate it.
- **Non-goals** — what is deliberately outside this change.
- **Domain impact** — objects, invariants, commands, queries, and serialization affected.
- **Interfaces** — concrete crate/module/type/function names expected at completion.
- **Milestones** — ordered, independently verifiable steps.
- **Acceptance** — commands/tests plus expected observable behavior.
- **Progress** — checkboxes updated whenever work advances or stops.
- **Decision log** — design choices and rationale, especially compatibility tradeoffs.
- **Discoveries** — unexpected facts found during implementation.
- **Outcome / remaining work** — what actually shipped and what did not.

Plans should describe behavior, not merely files to edit. Update the plan when reality differs from
the original assumptions; never leave a stale plan that claims completed behavior that does not
exist.
