---
name: make-the-change-easy
description: A change fights the existing code — it needs edits scattered across many places, a flag threaded through, or a special case wedged in. Refactor first so the change becomes one small edit, then make it.
---

# Make the change easy, then make the easy change

Kent Beck: "for each desired change, make the change easy (warning: this may be hard), then make the
easy change." Two moves, kept apart: a refactor that changes no behaviour, then a small change.

1. Write down the edit you wish you could make: the one-place change.
2. Name what blocks it — duplicated logic, the wrong module owning it, two phases tangled together.
   `smell-to-refactoring` maps each blocker to a named refactoring.
3. Refactor in small steps, tests green after each one, tests themselves unchanged.
4. Make the change, now one small edit, test first.

Done when: the refactor steps change no test and leave every test green, and the change itself is a
small, local diff that lands as its own commit.

Yields to: surgical scope — refactor only what this change touches; anything else becomes a separate
task.

In the wild:
- Martin Fowler, [An example of preparatory refactoring](https://martinfowler.com/articles/preparatory-refactoring-example.html) — a worked example of exactly these two moves.
- Fowler, *Refactoring* (2nd ed.), chapter 1 — the statement function is split into a calculation phase
  and a rendering phase (Split Phase) before HTML output is added, so the new feature is one new
  renderer.
