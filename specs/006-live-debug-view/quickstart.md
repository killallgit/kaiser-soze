# Quickstart

1. Tests: `task test:unit -- live::`, `task test:unit -- label::`, `task test:integration`, `task test:wrappers`, then `task check`.
2. Live: `task wrappers:install`, new shell, `task dev` in one terminal (lines print there), then `SOZE_CA_DIR=$PWD/target/dev/ca claude` or `codex exec --skip-git-repo-check "say hi"` in another. A request line and a reply line appear per call, ending with the directory.
3. Agent check (once per agent): run each of `claude`, `codex`, `agent` through the wrapper and note whether its lines show the command and directory. Record the result in the README.
4. Quiet: `LIVE=0 task dev`; the old `connect host:port` line is back and no other lines print.
5. Privacy: run a call whose prompt contains a distinctive word and grep the `task dev` output for it; it must not appear. Report counts only.
