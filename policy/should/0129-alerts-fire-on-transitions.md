# 0129 An alert reports a transition, not a state

Level: SHOULD
Scope: every alerting path — feed events, alarm events (`goal_alarm`, `log_volume_alarm`, and
their successors), escalations, and any future mechanism that asks for operator or agent
attention.

An alert SHOULD fire when the monitored condition changes, not when it merely persists.
Re-evaluating a condition that is still true is a gauge read, not news; re-emitting it turns
the alert channel into background noise that trains every reader to skip it. The division of
labor follows should/0106: the persisting condition lives in a gauge (and is rendered as a
standing line wherever the operator already looks, for example the viewer header), while the
event channel carries only these transitions:

- first crossing into the bad state (once);
- material worsening, defined as discrete tiers fixed at design time (for example each
  further gigabyte, each further failing test), one event per tier crossed;
- recovery back to ok, with hysteresis below the entry threshold so a boundary value cannot
  flap (one closing event — closure is information too);
- for inherently bursty conditions (rates, ratios), a cooldown between events instead of
  tiers;
- a bounded reminder (for example once per 24 hours) while the bad state persists, so
  "crossed once, then silent forever" cannot happen either.

Each alert names its consumer at design time (should/0111), and the transition table is
pinned by tests: same-state re-evaluation stays silent, a tier crossing fires, a rate
re-crossing inside the cooldown stays silent, the reminder fires only after its interval.

Rationale: attention is the scarce resource the alert spends, and repetition without new
information spends it for nothing — the same mechanism as prompt emphasis (should/0124):
accumulated identical signals cancel each other. Measured on 2026-07-30 (12h, w3): 21
escalation events and 93 stuck events produced zero behaviour change, while a single
NOT-served fact line changed behaviour within an hour — readers act on novelty, not on
volume. Designed first for `log_volume_alarm` (operator ruling 2026-07-30: transitions only,
1 GB tiers, 6 hour rate cooldown, 10 percent hysteresis, 24 hour reminder, standing gauge in
the viewer header).

Deviation: a condition whose every occurrence is independently actionable (a failed backup, a
security refusal) is a discrete event, not a state — it may fire each time; say so at the
emitting site. Genuinely critical conditions may repeat louder, with the justification
recorded.

Enforcement: convention + review; new alarm emitters cite this policy and ship their
transition-table tests alongside (must/0014).
