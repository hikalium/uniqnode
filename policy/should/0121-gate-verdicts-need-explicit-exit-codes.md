# 0121 Gate verdicts must come from explicit per-step exit codes, never through a pipe

Level: SHOULD
Scope: every shell chain whose outcome gates a consequential action — committing, deploying,
marking a check green, proceeding past a checkpoint — and every watcher whose silence is read as
information: a monitoring grep, an alert, a completion notifier. Companion to should/0116 (verify
by observed effect) and the general rule "verify the effect, not the exit code" — this is its
inverse trap: an exit code that never reaches the judge.

A pipeline's exit status is the last command's, so `checker | tail` (or `| grep`, `| head`)
returns the filter's success and silently swallows the checker's failure. A gate built that way
approves red results.

- Capture each step's status explicitly (`cmd > log 2>&1; status=$?`) and judge on `status`,
  or enable `set -o pipefail` for the chain. Filter the output from the saved log, not inline.
- After building a gate, make it fail once on purpose and confirm it actually blocks — a gate
  that has never been seen red is unverified.
- The same holds for a watcher before its silence is trusted. Trigger the condition once and
  confirm it fires. A watcher that never fires and a condition that never occurred are
  indistinguishable, so an unproven watcher makes every absence claim drawn from it worthless
  (should/0128). Do not discard stderr while proving it — a discarded error is how a watcher wired
  to a flag that does not exist stays silent.

Rationale (measured 2026-07-19): during MAIN-SPLIT, `cargo clippy -- -D warnings | tail -2 &&
git commit` produced a commit while clippy was red — the pipe returned `tail`'s 0. The defect
was caught before push and repaired by amend, but only by accident of reading the output.

Rationale for the watcher half (measured 2026-07-19 and 2026-07-30): a monitoring grep for a steer
marker was built on `ctl log --limit`, a flag that does not exist, with stderr discarded, so it ran
the whole watch without ever firing and its silence was read as "the event has not happened". Two
notifier commands of the shape `timeout ...; echo $?` reported success unconditionally, because the
reported status belonged to `echo` rather than to the watched command. Both are this policy's own
trap in a different costume: the status never reaches the judge.

Enforcement: convention; review of gate scripts and of any watcher whose silence will be reported
as a finding.
