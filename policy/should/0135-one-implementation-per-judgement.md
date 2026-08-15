# 0135 One judgement, one implementation, called from every site that needs it

Level: SHOULD
Scope: every judgement, conversion or predicate the machine is expected to answer the same way at
more than one site — Rust code in this repository, and the design documents that record where such
an answer lives. Neighbour to two rules and deliberately distinct from both: must/0023 covers the
case where the shared thing is a literal string, one site composing it and another matching on it;
should/0122 covers which module path an already-single item is reached through, not whether the item
is duplicated. This policy covers the answer itself, whatever its type.

When two sites need the same answer, the deciding code SHOULD exist in one place, and every other
site SHOULD call it rather than work the answer out again.

- The single home is named for the question it answers and lives with the code that owns that
  question (`toolset::tool_is_usable`, `audit::auditor_authorizes_run`, `knowledge::served_twin_of`,
  `uitest::parse_looks`, `docgate::code_span_mask`). Callers reach it by that name.
- The rule is about the answer, not its type: a boolean predicate, a parse, a normalisation, a set
  membership, a path derivation, an ordering. If the shared thing is the wording of a marker, the
  case is must/0023's and its constant rule applies there. If the question is only which path an
  already-single item is imported through, the case is should/0122's.
- A second implementation that agrees today is still the defect, because the two are corrected
  separately. The measured shape is always the same: someone finds a hole, fixes the copy in front
  of them, and the other copy keeps answering the old way with nothing recording the disagreement.
- A design document that says where a judgement lives is part of the same rule: it SHOULD name the
  single home, so the next reader extends that one instead of writing a second.
- Where a second implementation is kept on purpose, the two SHOULD be tied together by a test that
  runs both and asserts they agree, and the reason SHOULD be recorded at the deviating site.

Rationale: three measurements, two of them the failure and one the payoff.

A prompt site re-deriving "the `shell` grant is what makes `run` callable" (measured 2026-08-03):
the verify tip chose its follow-up command from the file extension and told the task to compile,
while the executor's own answer for that task was no. reversi0 held only the `device` grant and was
sent to `run` after every successful edit; the grant-based re-derivation is wrong in the other
direction too, because a role can hold the `shell` grant and still not carry `run` in its tool list,
which is exactly the shape of the golden fixture. The correction was not a better re-derivation but
the removal of one: `toolset::tool_is_usable`, the question the executor already asks, called from
the prompt site (container/src/toolset.rs, container/src/prompt.rs).

Two sides of one gate deciding separately whether an auditor counts (measured 2026-08-04): the side
that advertises the tool list and the side that refuses the call both have to know whether a task's
auditor stands in for the `shell` grant. Answered twice, the pair drifts into advertising a `run`
that is then denied, and the task spends steps on a tool it was told it had. Both sides now read
`audit::task_auditor` and the same `decides()` verdict, the advertising side through
`audit::auditor_authorizes_run` (container/src/toolset.rs, container/src/broker.rs).

The same shape paying off, because the implementation was already single (measured 2026-08-03): the
emphasis check behind `host check-docs` and the rewriter behind `host fix-emphasis` are driven by
one engine and therefore share `docgate::code_span_mask`. A mid-line backtick run of three or more
was being paired as an inline-code delimiter, which exempted every byte between two such mentions
from the prose checks; one table row carried 5 emphasis spans through every gate run for a day.
Closing the hole in the mask once moved the check and the fixer together. Two masks would have left
the fixer emitting files the check still failed (container/src/docgate.rs).

Filed as should/ rather than must/ because a second implementation is sometimes the point. A test
that pins deterministic behaviour by comparing the production path against a deliberately simple
reference implementation is the standard case, and it is worth nothing if it calls the code it is
meant to check. What is stated here is the default such cases deviate from, with the agreement test
and the recorded reason that keep the deviation honest.

Enforcement: convention; review. The mechanical half that exists is per-instance rather than
general: `prompt_advertises_only_usable_tools` (container/src/toolset.rs) fails when the advertised
tool list and the executor's gate disagree, which is what one re-derivation of that judgement
produces. For a plan or design that places a judgement, the applied form is viewpoint B of
[docs/mop/REVIEW-PLAN-DOC.md](#e25c9f99-9517-4020-ab09-62eb34e2b75f), where a claim about where an
answer lives is checked against the tree. No scanner can tell a second implementation from an
unrelated function, so the rest is review.
