# 0139 Write a reset as the list of what survives

Level: SHOULD
Scope: every boundary that promises a clean slate to whoever comes next — a measured trial, a test
fixture, a reused agent seat, a recreated work directory, a cache generation. Sibling of
should/0140 (store a ledger in the same region as the work it records): this policy governs how the
boundary is written, that one governs the lifetime the boundary then fixes for a ledger — a
delivered marker, a dedupe set, a seen list, a cursor — stored on one side of it. Sibling of
should/0134 (cross-task separation by construction): that policy governs what a reader is handed by
a path built for a task other than its own, this one governs what an occupant is handed by the
previous occupant of the same directory. Neighbour of should/0132, which controls a performance
measurement's confounders but says nothing about the state one repeated trial inherits from the
trial before it.

A reset SHOULD be written as a survivor list: destroy the whole region, then restore the entries a
named list says may cross. Enumerating what to delete instead puts the burden on the next person who
adds state somewhere near, and nothing in the tree tells them the list exists.

Concretely, when you build a boundary that promises a clean slate:

- Sweep and rebuild: remove the directory that holds the state, recreate it, and restore only the
  entries on the survivor list. State that somebody adds next month is then destroyed by default,
  which is the direction that fails safely.
- Write the survivor list at the reset itself, one entry per line, each with a one-line reason for
  why it may cross the boundary — an affinity hint the next occupant needs, a counter the operator
  reads between runs. Those reasons are the recorded should-level deviations.
- Choose the home of new state so it lands inside the swept region by default, and give the entries
  that must cross a directory of their own. Two regions with plain names outlive one region with a
  growing exception list.
- Pin the boundary with a test: seed the region with a file no rule mentions, run the reset, assert
  the file is gone (prove that test per should/0137 by running it against the enumerating reset).
  State added later outside the survivor list then fails that test instead of quietly riding into
  the next occupant's turn.
- List every write under a shared workspace's root before trusting trials that repeat there — grep
  the writers rather than recalling them — and check each path against the survivor list. Whatever
  the first trial leaves behind is part of what the second trial measures.

Rationale (measured 2026-08-10 over the 2026-08-09 sweep; full accounting in
[docs/analysis/20260810-trial-isolation.md](#8eace134-aa39-4903-9c3a-2f2dd43e4072)). The bench
harness reset a seat by naming seven things to delete — `memory/`, `.finished`, `.paused`,
`.verifying`, `usage.json`, `summary.md`, `build_broken` — and by recreating the task's `work/`.
Everything else under `agents/<seat>/state/` and the task's own `state/` crossed the trial boundary,
and since a seat is handed to whichever model the queue offers next, it crossed the model boundary
too. Of the ten entries in `agents/bench0/state/` at audit time, one was on the delete list, one is
rewritten by the harness right after the wipe, and eight survived unnamed. Six carry-over paths were
measured. `state/active_file` carried the previous trial's file into the first prompt of the next
one, under a note that says the full file is already in hand and `read` should not be called on it
again: on `tool-edit-line` and `tool-grep-fix` together, rep1 passed 47.4 percent (37/78) while rep2
and rep3 passed 65.4 percent (102/156), a gap of 18.0 points, and the six tasks with no leak were
flat across reps. A seventh task leaked at a comparable rate and its pass rate did not move, so no
row says whether its own leak mattered, which is why the sweep cannot be corrected after the fact
either. The heaviest of the six ran the other way: the answers queued in the task's `state/pending/`
survived the reset while the ledger recording their delivery did not, so they arrived again as new
work every trial — that path is a ledger stored outside its subject's region, and its measurements
are filed with should/0140. The seat's lifetime counter `uncommitted` reached 1875 on the busiest
seat and 40 on the quietest, and put "N code changes since your last commit" into the prompt every
third change; 338 rows had that tip and 303 of them (89.6 percent) fired while the trial's own
landed changes were under three.
The `active_file` path had been live since 2026-06-28, so every multi-rep sweep since then is
affected. 1,363 rows and 30.1 hours of fleet time stopped being a measurement of models — the share
of that total the ledger path carries is broken out in should/0140 — and the only route left is to
fix the boundary and re-run, at 10.12 hours of wall clock on five servers.

Filed as should/ rather than must/ because carry-over is correct where the occupant is continuous:
on a live worker seat `uncommitted` really is the number of uncommitted changes, and a queued answer
really is one that task is still waiting for. What is defective is a boundary that announces a clean
slate and does not deliver one. This is the default such a boundary is measured against, with the
survivor list and its reasons kept where a reviewer can read them.

Enforcement: convention at the site that builds the clean room, and review of any change that adds
state under a region something else resets. The mechanical form asked for above does not exist yet:
`wipe_agent` (container/src/bench.rs) still enumerates what to delete, so a test that seeds an
unexpected file in the seat root and asserts the reset removed it fails by construction —
`wipe_agent` names seven paths and touches nothing else. Inverting that reset and landing the test
together is the first boundary this policy becomes checkable on; prove the test by running it
against the enumerating reset first (should/0137). No scanner can tell a reset from any other
deletion, so the survivor list and the reasons on it stay review territory.
