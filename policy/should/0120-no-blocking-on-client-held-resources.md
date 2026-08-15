# 0120 Never block a completion-path operation on resources its clients hold

Level: SHOULD
Scope: supervisory operations that run on another activity's completion path — verification,
scoring, cleanup, finalization — and consume limited resources.

An operation that an activity must pass through to finish must never block waiting for a
resource that activities of the same class hold: the holder may be waiting for exactly that
operation, which is a circular wait. In preference order: (1) reuse the allocation already owned
by the activity being served; (2) opportunistically take a genuinely idle resource, with a
bounded lease; (3) proceed degraded — share, queue behind, or skip the reservation — but never
wait indefinitely. Liveness outranks strict resource accounting on the completion path.

Rationale (measured 2026-07): a verifier that "waits for a free server" deadlocks the moment all
servers are held by workers awaiting verification; riding the requester's own allocation removed
the cycle by construction.
Enforcement: review of any operation that sits on a completion path — the reviewer asks which
resource it waits for and who holds that resource. If the answer is "the activities it serves", the
wait is the cycle.
