# 0128 Before claiming absence, check the observation channel's blind spots

Level: SHOULD
Scope: anyone deriving claims from runtime logs and instruments — human operators, top-level
assistants and their subagents, and plan/design documents that cite motivating numbers.

A claim of absence — "X never fired", "X was never delivered", "0 occurrences" — SHOULD only be
made after confirming the channel used for counting can actually observe X. Known blind spots
that have already produced false absence claims: `ctl log` display truncation cuts long event
lines, digest fields are truncated at source (the uitest event keeps 300 chars), filters and
`--ev` selections narrow what is visible, and gauges sample a moment rather than a history.
When a blind spot could plausibly swallow X, verify against the raw payload (state files, tool
response bodies, artifact files) before asserting the zero — and say which channel the claim
is based on, so a reviewer can judge its reach.

Positive claims ("X happened 44 times") are cheaper to trust: a hit proves observability.
Absence claims carry the full weight of the channel's blind spots, and they are exactly the
claims that steer priorities — a plan motivated by "the model was never told" argues for a
different fix than "the model was told and did not act".

Rationale: during the 2026-07-30 pianoapp4 analysis the assisting session asserted "blank
reached the model 0 times via the harness" from `ctl log` greps; the raw uitest digest in fact
appended `LOOKS: blank page` beyond the log line's truncation point. The corrected claim — the
observation tail carried it, the decision-point block dropped it — pointed at a different (and
correct) fix. This policy is the complement of should/0126: reading logs through the command
is right, and the command's own truncation is still a blind spot to account for.

Deviation: exploratory notes may flag a tentative zero; mark it as unverified ("no hits in the
truncated view") rather than stating it as fact.

Enforcement: convention (operator/assistant discipline); plan documents citing absence numbers
name the channel checked.
