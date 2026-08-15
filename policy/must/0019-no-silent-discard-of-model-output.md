# 0019 Never silently discard model output on a format mismatch

Level: MUST
Scope: every harness site that machine-reads LLM output — tool-call parsers, body/fence
handling, judge/vision/witness answer readers, and any future consumer of generated text.

A parser of model output MUST NOT let content vanish because it failed to parse. When the
output does not match the expected shape, the site must do one of:

- fail open — pass the raw text through to the downstream consumer unmodified (mark it as
  unparsed if the distinction matters), or
- reject visibly — emit an explicit rejection the model can see (a tool-response message
  naming what was expected) plus a log event, so the miss is observable and countable.

Adding a new path where "parse failed → the result is quietly empty" is a policy violation
regardless of how unlikely the mismatch seems.

Rationale: the same accident shipped independently at least three times before this rule.
write's fence-contract rejection discarded 44 intact full-page bodies in 12h while letting only
stubs land (2026-07-29, the substance-collapse trigger); edit discarded intact inline bodies in
55% of its failures (2026-07-02); the looks_block bullet filter dropped the vision verdict
"blank page" for 12h, so the one actor who could fix a visually blank page was the only actor
not told about it (2026-07-30). Every fix was the same fix — fail open or reject visibly — and
each incident cost hours to days of agent thrash before a human found the drop. Silence is the
expensive part: a visible rejection is recoverable in one turn; a quiet empty result is
indistinguishable from "nothing to report".

Enforcement: code review; new parser sites reference this policy at the mismatch branch. Tests
for a parser site must include a nonconforming-input case asserting the content survives (fail
open) or the rejection is explicit.
