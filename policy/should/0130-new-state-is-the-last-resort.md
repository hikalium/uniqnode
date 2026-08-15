# 0130 A new state or status is the last resort, not the first fix

Level: SHOULD
Scope: every state machine in this system — task and episode status, agent state, condition
dimensions, gate verdicts — and any design that proposes to handle an anomaly by adding one.

When an anomaly needs handling, a new status or state SHOULD NOT be the proposed fix until the
cheaper routes have been checked and found insufficient. Check them in this order and say which
one failed:

- fix the quality of the input, so the anomaly stops being produced;
- absorb it on the consuming side — smooth it, reword it, treat it as an existing case;
- measure it and observe, when the anomaly is rare enough that behaviour need not change yet;
- reuse an existing state that already means what the anomaly means.

Only when all four are shown insufficient does a new state become the right answer, and the
proposal then names the transitions it adds and the consumers it touches.

Rationale (operator ruling 2026-07-20): the proposal was that a witness producing zero accepted
citations should skip rating and record a new `witness_starved` status plus an alarm. It was
rejected on the ground that branches which complicate the state machine must be avoided wherever
possible, and the eventual fix reused the existing `abandoned` status instead. A state is not one
value: it multiplies the transition graph, and that multiplication propagates into resume,
cleanup, display, scheduling and every test that enumerates states. The cost is paid by every
future reader of the machine, while the anomaly that motivated it is usually a one-off.

Enforcement: convention; review of any change that adds a status value. The visible symptom to
look for is a status introduced together with the single condition that produces it.
