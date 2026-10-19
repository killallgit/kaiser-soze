# Feature Specification: Agent wrappers

**Status**: Draft
**Input**: User request: run plain `claude`, `codex` and Cursor's `agent` through the proxy without typing env vars, with scripts to install and remove the wrappers. If soze is broken, the agents must work exactly as they do today. Roadmap item 6 (shell functions part).

## Problem

Capturing an agent today means typing `HTTPS_PROXY=... CODEX_CA_CERTIFICATE=... codex ...` by hand, and `claude` needs `-p` in the examples. A person wants to type `claude` and have it captured when soze is running, and have it behave as if soze did not exist when it is not.

## User Story 1 (P1): Plain agent commands are captured when soze runs

After install, `claude`, `codex` and `agent` start through the proxy with no extra arguments, interactive or not.

**Independent test**: with a stub proxy listening and a stub agent on the path, calling the wrapper starts the agent with the proxy and CA variables set and every argument passed through unchanged.

**Acceptance**
1. Wrapper `claude` sets `HTTPS_PROXY` and `NODE_EXTRA_CA_CERTS`; `codex` sets `HTTPS_PROXY` and `CODEX_CA_CERTIFICATE`; `agent` sets `HTTPS_PROXY` only (its host is tunnelled, not decrypted).
2. Arguments, stdin, stdout, exit status and the terminal behave exactly as without the wrapper, including interactive use.
3. The variables are set for that one command only, never exported into the shell.
4. The port comes from `SOZE_PORT` and the CA from `SOZE_CA_DIR`, with fixed defaults matching `soze`'s defaults.

## User Story 2 (P1): A broken or absent soze never breaks an agent

**Independent test**: with nothing listening, a missing CA file, an unreadable config, or soze killed, the wrapper runs the real agent with the same environment as no wrapper.

**Acceptance**
1. If nothing accepts a connection on the proxy port, the agent runs untouched.
2. If the CA file is missing or unreadable, the agent runs untouched.
3. If the person already set `HTTPS_PROXY` (or `https_proxy`), the wrapper leaves their environment alone.
4. The check is one quick local connection test with a short timeout, so a dead soze adds no noticeable delay to start-up.
5. `SOZE_OFF=1` skips the wrapper for one command. `command claude` also skips it.
6. A wrapper bug, such as a missing helper, falls through to the real command, never to an error.
7. Known limit, written in the README: if soze dies after the agent started, that session loses its connection, because the proxy address is fixed at launch.

## User Story 3 (P2): Install and remove are safe and reversible

**Acceptance**
1. `task wrappers:install` writes one wrappers file under `~/.config/soze/` and adds one marked block to the shell start-up file (zsh and bash). Running it twice changes nothing the second time.
2. `task wrappers:uninstall` removes the block and the file, leaving the start-up file byte-identical to before install apart from that block. Uninstall with nothing installed succeeds and changes nothing.
3. Install shows what it will change and writes a backup of the start-up file first. `--dry-run` writes nothing.
4. Install and uninstall work against a throwaway home (`HOME` override), so tests never touch real files.
5. Nothing is written under `~/.claude`, `~/.codex` or `~/.cursor`, no CA is added to a trust store, and no credential store is opened.

## Requirements

- **FR-001**: One wrapper per agent command: `claude`, `codex`, `agent`.
- **FR-002**: Wrappers fail open (story 2). Every failure path ends in the real command, unchanged.
- **FR-003**: Wrappers add no output to the terminal.
- **FR-004**: Install and uninstall are idempotent, reversible and testable against a throwaway home.
- **FR-005**: Works in zsh and bash on macOS and Linux.
- **FR-006**: Tests run the wrappers with stub agents and a stub listener, assert on the environment seen and the exit status, and never run a real agent.

## Assumptions

- The person starts `soze` themselves (`task dev` or the binary) on the fixed port. A background service is roadmap item 6, out of scope here.
- Cursor's `agent` is tunnelled only, so it gets the proxy variable but nothing is parsed from it.
- Fish and PowerShell are out of scope.

## Out of scope

Background service, auto-starting soze from a wrapper, fixed-port handling in `soze` itself (it already has `--port`), run attribution.
