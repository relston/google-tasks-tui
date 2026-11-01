# Agent notes

Project architecture, safety rules, and verification live in
[`CLAUDE.md`](CLAUDE.md). Work directly on `main`.

## After every complete change

When a task or coherent set of edits is finished and the checks in
`CLAUDE.md` pass:

1. Install the local binary: `cargo install --path .`
2. Commit on `main` (include `TASKS.md` / `CHANGELOG.md` updates in the
   same commit as the work).
3. Push to `origin`.

Do not leave finished work uninstalled, uncommitted, or unpushed.
