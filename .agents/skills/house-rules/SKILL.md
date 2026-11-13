---
name: house-rules
description: House rules for writing, changing, or reviewing code — simplicity, deletion, naming, comments, tests, verification. Load before touching code.
---

# House rules

Write code a stranger understands in one read. Every rule below serves that reader.

## Core

1. **Simplify.** Build the smallest design that fully solves the problem in hand. A third real case
   earns a generalization (rule of three).
2. **Delete.** Remove what nothing needs, including a function only its own test calls — delete the
   function and the test together. Git keeps the history.
3. **Comments explain why.** Names and types carry the what. A comment earns its place for a
   constraint, a non-obvious reason, or a gotcha. Model: antirez, [Writing system software: code
   comments](http://antirez.com/news/124).
4. **Verify.** Treat agent output, your own included, as an unreviewed draft: run it, test it, and
   read the diff against the repository before calling it done.
5. **Read like prose.** Names, file layout, and test names tell the story of what the code is for.
   Optimize for a first-time reader's time to understanding; trade readability for speed only when a
   measurement asks for it.

## Design

- **Deep modules.** A small interface with substantial behaviour behind it. Inside a module, each
  function does one job and is named for it; split a function when it holds two jobs, not to reach a
  line count. This sides with Ousterhout where he and Clean Code disagree — see
  [aposd-vs-clean-code](https://github.com/johnousterhout/aposd-vs-clean-code).
- **Functional core, imperative shell.** Decisions live in pure functions; IO stays at the edges;
  dependencies arrive as arguments.
- **Encapsulated state.** State lives inside the module that owns it. Global mutable state needs the
  user's sign-off and a stated reason.
- **Few arguments.** Three or four at most; past that, group related values into an immutable
  parameter object.
- **Interfaces at seams.** Depend on an interface where a second implementation exists — a test double
  counts as one.
- **Named domains.** A `utils` module signals a missing domain concept: name the concept and give it a
  home.

## Tests

- Test first: a test that fails for the intended reason, then the code that makes it pass.
- Assemble, Act, Assert, one behaviour per test. A one-line comment states the assertion.
- Assert on behaviour, values and concrete error types, never on message text.
- Unit and integration tests live apart, each runnable on its own.

Examples per language: [Rust](references/rust.md), [Go](references/go.md),
[Python](references/python.md), [TypeScript](references/typescript.md).

## Gates

- Lint, typecheck, and tests pass before a push.
- A latent failure your change exposes is fixed in the same work, or handed to a parallel agent when
  speed matters.
- Docs record intent and contracts, the parts that change slowly; volatile detail stays in the code.

## Principle skills

Load the matching skill when its moment arrives:

| Moment | Skill |
|---|---|
| Input crosses a trust boundary | `parse-dont-validate` |
| A change fights the existing code | `make-the-change-easy` |
| Reviewing a diff, or code feels wrong | `smell-to-refactoring` |
