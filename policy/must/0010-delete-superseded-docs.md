# 0010 Delete superseded documentation — never leave stale descriptions in the tree

Level: MUST
Scope: all in-tree prose that describes the system's design or behaviour — `DESIGN.md`, `docs/`, `policy/`,
plan files, and explanatory comments. (NOT the git history.)

When a design or behaviour changes, the old description MUST be deleted from the working tree — removed
outright. Do NOT keep it, do NOT mark it "superseded / historical / deprecated", do NOT comment it out.

- Git preserves every past version; the working tree is the single current truth. A reader — human or agent
  — must be able to trust that what is in the tree is what is true now.
- Stale descriptions pollute context and mislead: an agent that reads a superseded mechanism may implement
  or reason from it. That is actively harmful, not mere clutter.
- A completed or superseded plan file is deleted, not retained as a record (`git log`/`git show` is the
  record).
- Replace, don't accumulate: when something changes, edit the description in place to the current truth; if
  a whole document is obsolete, remove the file.

Code comments follow the same rule. Every comment — except a TODO marker for future work —
describes the present: the current behaviour, its constraints, and distilled facts that justify it
(a measurement result is fine; a chronicle is not). Comments MUST NOT narrate provenance or change
history（「旧 X の置換」「was previously …」「moved from …」）— that describes the diff, not the
code, and git already records it. When behaviour changes, rewrite the comment in the present tense
and delete what no longer applies.

The one exception — a retired policy. Deleting a `policy/` file outright would leave every
reference to its number pointing at nothing, with no way for a reader to learn where the rule went,
and would free the number for reuse. A retired policy therefore keeps its file, renamed to
`policy/<level>/NNNN-obsoleted.md`, so the retirement is visible in the file name and in the title
the policy tools print. Its whole content is the line

    This policy is obsoleted. Remove reference to this policy.

followed by nothing but the ids of the policies the rule moved into (one per line, or none if it was
withdrawn outright). The old body is deleted like any other superseded text — what remains is a
redirect, not a description, and the instruction on the first line is an obligation on whoever finds
it: repoint the reference and the stub stops being read. This is the only place in the tree where a
superseded artifact is kept instead of removed.

Rationale: the working tree is a gauge of the current design (cf. should/0106 for runtime state); the past
lives in git, never in the tree. The retirement stub is not an exception to that — it carries no
description of the retired rule, only its number and its successors.
Enforcement: review, for prose in general: no checker can tell a stale description from a current
one. The retirement stub is mechanical — `cargo test` self-test `the_policy_corpus_is_well_formed`
(container/src/policy.rs) rejects a stub that kept any of its old body or a wrong first line, and
fails on any surviving reference to a retired policy, so "remove reference to this policy" is
enforced rather than requested.
