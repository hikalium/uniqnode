# 0104 No unjustified hardcoded waits

Level: SHOULD
Scope: all lamalium code — the supervisor, agents, and host tooling (`container/src/*.rs`).

Do NOT add a fixed `sleep`/delay as a stand-in for waiting on a real condition. A wait should be
deterministic: poll a definite condition (a file appears, a state flips, a log event lands, a port
answers) and proceed the instant it holds — not after a guessed interval. A `sleep` that always elapses
regardless of the thing it is "waiting" for is a smell: it is both slow (waits when it needn't) and
flaky (too short under load, too long otherwise). It also hides a missing synchronization point.

Acceptable timed waits — each needs a short written justification at the call site:
- a control-loop tick / poll interval (an event loop with no inotify yet): the cadence at which a
  condition is re-checked — e.g. the supervisor's `sleep(2)` between bus sweeps;
- a safety timeout that bounds a condition-poll so it cannot hang forever on a wedged/crashed peer —
  paired with early return on success and a loud failure (not a silent proceed) on expiry;
- an externally-mandated settle time (hardware/protocol) with a cited source.

Not acceptable: `sleep(N)` "to let it settle" / "to be safe" with no condition and no rationale; a
magic-number timeout with no explanation of where the number comes from; padding that masks a race.

Prefer, in order: (1) remove the wait by making the blocking dependency asynchronous or parallel so
the caller need not wait at all; (2) wait on the exact condition with a fine poll interval; (3) if a
bound is genuinely needed, make it a safety net with a justification, and fail loudly on expiry.

Rationale: deterministic, condition-driven synchronization is faster and far less flaky than guessed
delays, and it documents the actual dependency between steps instead of hiding it behind a number.

Enforcement: code review. A reviewer who sees a bare `sleep`/fixed delay asks: "what condition does this
actually wait for, and why can't we wait on that?" If there is no good answer, the wait does not ship.
