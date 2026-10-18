# Data Model: Dev and Test Commands

No stored data. These are the named things the feature defines.

## Task

| Field | Meaning |
|-------|---------|
| name | `check`, `test:unit`, `test:integration`, `dev` |
| purpose | One line, shown by `task --list` and the README |
| exit status | 0 on success, non-zero on any failure |
| time bound | `dev` stops within the constitution limit; the other tasks end on their own |

## Script

A file under `scripts/` called by exactly one task. A task body longer than a few lines becomes a script. A script never calls another script.

## Check

One step inside `task check`: `fmt`, `clippy`, `test`. A check is passed or failed. `task check` reports the set of failed checks.

## CI run

One execution of `task check` on a pull request or a push to `main`. Result: pass or fail, shown on the pull request.

## Relationships

- `check` calls `scripts/check.sh`, which runs the three checks.
- `dev` calls `scripts/dev.sh`.
- `test:unit` and `test:integration` are single-line tasks with no script.
- A CI run executes `check`.
