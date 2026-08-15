# 0140 Store a ledger in the same region as the work it records

Level: SHOULD
Scope: every ledger — every record whose content is "this piece of work has already been handled":
a delivered marker, a dedupe set, a seen list, a cursor, a watermark, a done file. Sibling of
should/0139 (write a reset as the list of what survives): that policy governs how a boundary is
written, this one governs the lifetime that boundary then fixes for the ledger, because whichever
region a ledger is stored in, the reset of that region is when it dies. Neighbour of should/0130
(new state is the last resort), which asks whether the record should exist at all; this one starts
after that question is settled and asks only where it lives.

A ledger and the work it describes have one lifetime between them. A ledger SHOULD be stored in the
region its subject lives in, so that the two are destroyed together or survive together, and a
ledger that must outlive or underlive its subject SHOULD carry, at its own site, the reason it is
allowed to. A ledger shorter-lived than its subject makes the subject arrive as if it were new,
over and over.

Concretely, when a ledger exists or is being added:

- Store every ledger in the region its subject lives in. A record that work has already been handled
  belongs beside the work it describes: if the queued items survive the reset, keep the delivered
  marker with them; if the items are destroyed, destroy the marker in that sweep.
- Name both resets before you choose the path. Say which sweep destroys the subject and which sweep
  destroys the ledger; if the two answers are different sweeps, the ledger is in the wrong place,
  and the direction of the mismatch tells you which failure you bought — repeated work when the
  ledger dies first, silently skipped work when the subject dies first.
- Write the reason on the ledger when it genuinely has to cross a region boundary — a comment beside
  the path saying which reset it is expected to survive and why its subject's reset does not govern
  it. That note is the recorded should-level deviation, and it is what the next reader checks
  against.
- Prefer a ledger the subject already carries: a field in the record, a rename of the item itself, a
  move into a handled directory beside it. State that cannot be separated from its subject cannot
  outlive it either.
- Pin the boundary with a test: seed one item, record it as handled, run the reset the production
  path runs, and assert the item is not handled a second time (prove that test per should/0137 by
  running it against the current placement first).
- Grep the writers of both paths before trusting a run that repeats — the code that writes the
  ledger and the code that wipes the region — rather than recalling where either lives.

Rationale (measured 2026-08-10 over the 2026-08-09 sweep; full accounting in
[docs/analysis/20260810-trial-isolation.md](#8eace134-aa39-4903-9c3a-2f2dd43e4072)). The bench
harness stored the ledger of already-delivered answers at `DELIVERED_LEDGER` =
`work/target/.lamalium-pending-delivered`, inside the task's `work/` directory, which the harness
deletes at the start of every trial. The answers it records live in the task's `state/pending/`,
which the harness keeps. Putting the marker somewhere the agent can write to was a defensible
choice on its own; storing it one region away from its subject is what made the answers survive
while the record of having delivered them did not, so every queued answer was re-delivered as a
synthetic first step, trial after trial. 393 of 1,353 matched rows (29.0 percent) took such a
re-delivery, 167 of 346 (48.3 percent) in the fact-only run and 108 of 117 on one task; rep1 was
hit as well (76 of 339, and 56 of 117), so keeping only rep1 does not rescue the data. One expired
question was re-delivered into 160 sessions across 31 models over 26.1 hours — a stale answer
telling the model it had no permission to read, handed to models that were asked to read a file.
The whole sweep, this path and the seat-side carry-over of should/0139 together, stopped being a
measurement of models.

Filed as should/ rather than must/ because a ledger that outlives its subject's region is right
where the occupant is continuous: on a live worker seat a marker that a question was already
answered should survive the recreation of a work directory, and a cursor into an append-only log
outlives every generation of the cache it feeds. What is defective is a lifetime nobody chose —
a ledger that landed in whichever directory was convenient and inherited that directory's reset.
This is the default such a placement is measured against, with the reason for any crossing kept
where a reviewer reads it.

Enforcement: convention at the site that adds the ledger, and review of any change that writes a
marker under a region something else recreates. Nothing mechanical exists: no check relates the
path a ledger is written to with the path its subject is written to, and no scanner can tell a
ledger from any other file. Both ends of the measured instance are in the tree and separately
tested — `mark_delivered` and `due_deliveries` in container/src/delivery.rs, whose tests
the_delivered_ledger_is_bounded_and_a_trimmed_mark_never_redelivers and
a_delivered_ledger_that_cannot_be_read_is_reported_instead_of_read_as_empty pin the ledger's
content and never its address, and `wipe_agent` in container/src/bench.rs, which removes the region
holding it. The test asked for above is the first place this becomes checkable: seed an answer,
deliver it, run the harness reset, assert it is not delivered again. The mechanization worth
building is a hygiene check that reads every path constant naming a marker file and reports the
ones whose directory is recreated by a reset the same crate performs.
