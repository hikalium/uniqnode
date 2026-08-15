# 0024 Stage commits by explicit path — never blanket-stage

Level: MUST
Scope: every git commit made in this repository — by humans, assistants, or agents — and any
tooling that stages files on their behalf.

The content of a commit MUST be enumerated intentionally: stage by explicit path
(`git add <path> ...`), review `git status --short` and `git diff --cached --stat` before
committing, and commit only what the change requires. Blanket staging MUST NOT be used:
`git add -A`, `git add .`, `git add --all`, `git add -u` over the repository root, and
`git commit -a` are all prohibited. A deletion is staged by naming it too (`git rm <path>` or
`git add <deleted-path>`).

Rationale: a working tree holds more than the change being delivered — session scratch,
editor state, embedded repositories, generated files not yet ignored. `.gitignore` covers only
what is already known to be foreign; blanket staging commits the unknown remainder by
construction, and the mistake surfaces only after push, where history is not rewritten
(operator rule). Measured instance 2026-08-06: `git add -A` swept eight
`.claude/worktrees/agent-*` gitlink entries into commit 280a38b and pushed them. No file
content leaked — a gitlink is a bare commit pointer, and the targets were this repository's
own commits — but the tree carried eight foreign entries, the cleanup cost a follow-up commit
(36dc0ea), and the pointer lines remain in pushed history permanently. Operator ruling
2026-08-06: record the prohibition as policy.

Enforcement: convention. `.gitignore` keeps known-foreign paths (`.claude/`) out of status
output as mitigation, but ignoring is not a substitute for explicit staging — the rule exists
precisely for the paths nobody has ignored yet.
