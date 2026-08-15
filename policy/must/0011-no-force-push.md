# 0011 Never force-push or rewrite published history

Level: MUST
Scope: any git history that has been pushed to a remote — especially the default branch `main`.

Once a commit is pushed, its history is immutable. Never rewrite it.

- No `git push --force` / `--force-with-lease` to a remote branch.
- No amend / rebase / reset of an already-pushed commit followed by a push.
- Correct a mistake in a pushed commit with a new forward commit (or `git revert`) — never by rewriting.
- Local-only (not yet pushed) commits MAY be amended / rebased freely before their first push.

Rationale: rewriting published history is destructive — it silently breaks anyone (or any clone / worktree /
CI) holding the old history. A tidier graph is never worth a rewritten remote; a forward commit costs
nothing and keeps the record honest (cf. must/0010 — the tree is the current truth, git is the immutable
record).
Enforcement: review at the push site; may/0203 restates the constraint where Claude commits and
pushes without a per-action approval. Nothing on the remote is relied on to stop it — this invariant
is held by whoever runs the push.
