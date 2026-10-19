# Quickstart

1. Tests: `task test:wrappers`, then `task check`.
2. Try it in a throwaway home: `HOME=$(mktemp -d) task wrappers:install -- --dry-run`, then without `--dry-run`, then `wrappers:uninstall`; the start-up file in that home returns to its original bytes.
3. Real use: `task wrappers:install`, open a new shell, run `task dev` in another terminal, then `SOZE_CA_DIR=$PWD/target/dev/ca claude` in a third. `target/dev/tap.jsonl` starts growing.
4. Fail-open: stop `task dev` and run `claude` again; it starts as it did before install. Report counts only, never message content.
