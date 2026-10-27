# Feature Specification: Dev and Test Commands

**Feature Branch**: `001-dev-test-commands`

**Created**: 2026-10-18

**Status**: Draft

**Input**: User description: "Setting up proper dev and testing commands"

## Clarifications

### Session 2026-10-18

- Q: How are project commands defined? → A: In a Taskfile. A task body longer than a few lines moves to a script under `scripts/`.
- Q: What does the full check command run? → A: `task check` runs all linting, tests and other checks.
- Q: Is CI in scope? → A: Yes. CI runs the checks and tests.
- Q: What is the dev command called? → A: `task dev`.
- Q: How does CI run the checks? → A: With standard Rust build and test steps, not through Task. CI does not install Task.
- Q: What test framework do tests use? → A: The standard `cargo test` framework. No other test framework is added.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run every check with one command (Priority: P1)

A contributor (human or coding agent) finishes a change and runs `task check`. It runs all linting, tests and other checks, and reports pass or fail. Today the gate is three chained commands copied from `AGENTS.md`.

**Why this priority**: `task check` decides whether a change may be integrated. One command removes copy errors and gives every contributor and CI the same definition of "green".

**Independent Test**: Run `task check` on a clean checkout and see pass. Introduce a formatting error, a lint warning, and a failing test in turn. See fail each time.

**Acceptance Scenarios**:

1. **Given** a clean checkout, **When** the contributor runs `task check`, **Then** it exits with success.
2. **Given** a change with a formatting, lint or test failure, **When** the contributor runs `task check`, **Then** it exits with failure and names which check failed.
3. **Given** `task check`, **When** compared with the gate in `AGENTS.md`, **Then** it runs at least the same checks.

---

### User Story 2 - Run unit and integration tests separately (Priority: P2)

A contributor runs only the fast unit tests while editing, and runs the integration tests on their own when the proxy path changes.

**Why this priority**: The constitution requires unit and integration tests to be separate and separately runnable. Fast feedback while editing keeps the test-first loop short.

**Independent Test**: Run the unit task and confirm no integration test runs. Run the integration task and confirm no unit test runs.

**Acceptance Scenarios**:

1. **Given** the project, **When** the contributor runs the unit test task, **Then** only unit tests run.
2. **Given** the project, **When** the contributor runs the integration test task, **Then** only integration tests run.
3. **Given** a single module name, **When** the contributor passes it to the unit test task, **Then** only that module's tests run.

---

### User Story 3 - Start the proxy for local development (Priority: P2)

A contributor runs `task dev` to start the proxy locally, without remembering the proxy flags or where to put the tap and CA files. The contributor still sets the agent's proxy and CA environment variables by hand.

**Why this priority**: Real-agent checks are how adapters are proven. A mistyped setup risks writing into agent folders or running unbounded.

**Independent Test**: Run `task dev` and see the proxy listening address and CA path. Stop it. Confirm nothing was written under agent folders.

**Acceptance Scenarios**:

1. **Given** the project, **When** the contributor runs `task dev`, **Then** the proxy starts and prints its listening address and CA file path.
2. **Given** `task dev` is running, **When** the contributor stops it, **Then** it exits cleanly.
3. **Given** `task dev`, **When** it runs, **Then** it never installs the CA into a trust store and never writes under agent folders.

---

### User Story 4 - CI runs the same checks (Priority: P2)

When a change is pushed or a pull request is opened, CI runs the checks and tests automatically, running the same checks as `task check`, through standard Rust steps and not through Task. A merge can wait for CI to pass.

**Why this priority**: The repo has no CI today, and documentation and code changes are merged only after CI passes. A CI result that differs from the local result wastes time.

**Independent Test**: Open a pull request with a deliberate failing test and see CI fail. Fix it and see CI pass.

**Acceptance Scenarios**:

1. **Given** a pull request, **When** CI runs, **Then** it runs the formatting, lint and test checks and reports the result on the pull request.
2. **Given** a change that fails `task check` locally, **When** CI runs, **Then** CI fails too.
3. **Given** a change that passes `task check` locally, **When** CI runs, **Then** CI passes too.

---

### User Story 5 - Discover the commands (Priority: P3)

A new contributor learns which tasks exist and what each does from the README and `AGENTS.md`, without reading the Taskfile.

**Why this priority**: Commands nobody can find do not get used.

**Independent Test**: A reader who has never seen the repo lists every task and its purpose from the README alone.

**Acceptance Scenarios**:

1. **Given** the README, **When** a reader looks for how to test, **Then** `task check`, the unit and integration test tasks, and `task dev` are each listed with one line of purpose.
2. **Given** `AGENTS.md`, **When** its gate section is read, **Then** it names `task check` instead of repeating the chained checks.

---

### Edge Cases

- A check fails early. `task check` must still report every failing check in one run, so one run shows all fixes needed.
- `task dev` is a shared service. It runs until explicitly stopped; callers bound separate smoke tests rather than imposing a service lifetime.
- A task runs from a directory other than the repo root. Tasks must work from any directory inside the repo.
- Integration tests need no network or real credentials. They must pass offline, including in CI.
- A task body grows past a few lines. Its logic moves to a script under `scripts/`, and the task calls the script.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The project MUST define its commands in a Taskfile at the repo root.
- **FR-002**: A task body longer than a few lines MUST live in a script under `scripts/`, and the task MUST call that script.
- **FR-003**: `task check` MUST run the formatting check, the lint check with warnings as errors, and all tests.
- **FR-004**: `task check` MUST exit with failure if any check fails, and MUST report which checks failed.
- **FR-005**: The project MUST offer a unit test task that runs only unit tests, and an integration test task that runs only integration tests.
- **FR-006**: The unit test task MUST accept an optional module name that limits the run to that module.
- **FR-007**: `task dev` MUST start the proxy and print its listening address and CA file path.
- **FR-008**: `task dev` MUST NOT install the CA into a trust store and MUST NOT write under agent folders.
- **FR-009**: Every task MUST work from any directory inside the repo.
- **FR-010**: `task dev` MUST stay running until explicitly stopped, per the user's service-lifetime decision. Automated tests and smoke runs remain bounded by their callers.
- **FR-011**: CI MUST run on every pull request and on every push to `main`, and MUST report pass or fail on the pull request.
- **FR-012**: CI MUST run the same checks as `task check` (formatting, lint with warnings as errors, all tests) using standard Rust steps. CI MUST NOT install or call Task.
- **FR-013**: The README MUST list every task with one line of purpose. `AGENTS.md` MUST name `task check` in place of the chained checks.
- **FR-014**: Tests MUST use the standard `cargo test` framework. This feature MUST NOT add another test framework or test dependency.

### Key Entities

- **Task**: A named, documented entry point in the Taskfile for one job (check, unit tests, integration tests, dev proxy). Has a purpose, an exit status, and a time bound.
- **Script**: A file under `scripts/` that holds the logic of a task too long for the Taskfile.
- **Check**: One verification inside `task check`: formatting, lint, tests, or another project requirement.
- **CI run**: One automatic execution of `task check` for a pull request or a push.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A contributor runs every check with `task check`, and 100% of the gate checks in `AGENTS.md` are covered by it.
- **SC-002**: A new contributor finds and runs the unit tests within 1 minute of opening the README.
- **SC-003**: A unit-test run for a single module finishes without running any integration test (0 integration tests executed).
- **SC-004**: A first-time contributor starts the proxy for development with `task dev`, with 0 flags and 0 file paths typed.
- **SC-005**: A deliberate formatting, lint or test failure makes `task check` fail in 100% of 3 trial runs.
- **SC-006**: A deliberate failure on a pull request makes CI fail in 100% of 3 trial runs, and CI runs the same 3 checks as `task check` and no others.

## Assumptions

- Users are contributors to this repo and the coding agents that work in it.
- The three-step gate in `AGENTS.md` is exactly what `task check` runs. A further check, added later, goes into `task check` and CI together.
- Unit tests live beside the code. Integration tests live in the separate `tests/` folder. The split follows that layout.
- The repo is hosted on GitHub, so CI is a GitHub-hosted workflow. A different CI provider is out of scope.
- The Taskfile tool is installed by each contributor. CI does not use it.
- The CI check list and `task check` can drift apart, since they are two definitions. Reviewers compare them when either changes.
- No new Rust dependency is needed. Any addition needs the user's approval.
- The dev command's choice of agent and prompt stays manual. Pointing the agent at the proxy (its proxy and CA environment variables) also stays manual; only starting the proxy is automated.
