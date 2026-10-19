# Contract: wrappers and install

## Start-up file block (exact)

```
# >>> soze wrappers >>>
if [ -r "$HOME/.config/soze/wrappers.sh" ]; then . "$HOME/.config/soze/wrappers.sh"; fi
# <<< soze wrappers <<<
```

Install appends it once. Uninstall deletes the lines from the first marker to the second, inclusive. A start-up file with two blocks gets both removed.

## `scripts/wrappers.sh`

`wrappers.sh install|uninstall [--dry-run] [--yes] [--rc FILE]`. `SOZE_RC` also names the start-up file; `--rc` wins. Install asks `[y/N]` before editing the start-up file (only `y` or `yes` edits; `--yes` skips the question). If the file cannot be written, or the answer is not yes, install still installs the wrappers file, prints the block and how to name another file, and exits 0. Uninstall of a block in an unwritable file removes the wrappers file and prints which lines to delete. Exit 0 on success and when there is nothing to do; non-zero only for an unknown argument or a file it cannot write. `--dry-run` prints what would change (paths and the block, never other file content) and writes nothing. `HOME` is honoured, so a throwaway `HOME` isolates every path.

## Wrapper behaviour

- Ready: the agent runs with the variables in the data model table; arguments, stdin, stdout, stderr and exit status are the agent's own.
- Not ready: the agent runs with the caller's environment unchanged.
- The wrapper prints nothing of its own and exports nothing.
