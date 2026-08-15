# 0115 Check the worktree before large changes; prefer fine-grained commits

Level: SHOULD
Scope: any agent (or the top-level operator seat) about to start a substantial piece of work in a
git repository — refactors, reorganizations, multi-file features.

Before starting a large change, check the working tree state (`git status`). If uncommitted
changes are present that belong in a separate commit from the work about to start, commit them
first — then proceed. Never let two unrelated campaigns interleave in the same dirty tree.

Commit granularity: fine-grained beats coarse — small commits are easier to review, revert,
bisect, and cherry-pick. When unsure how to slice, choose the smaller commits, bounded below by
meaning: each commit stays one coherent unit of intent (don't split a single logical change into
fragments that can't stand alone).

Rationale (measured, 2026-07): an approved-but-uncommitted docs reorganization shared
`container/src/main.rs` / `host.rs` with a scheduler feature that landed afterwards — the
entangled tree then required hunk-level surgery to separate what one `git commit` per campaign
would have kept trivially apart.
Enforcement: self-review before starting the change (`git status` first, and commit what belongs to
the previous campaign); granularity is reviewed with the change itself.
