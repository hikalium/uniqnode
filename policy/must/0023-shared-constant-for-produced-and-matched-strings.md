# 0023 A string one site produces and another site matches on is a single shared constant

Level: MUST
Scope: all Rust source in this repository (`container/src/`, `container/tests/`,
`container/build.rs`) — every pair of sites where one composes text (a tool observation, a gate
marker, a rejection message) and another decides something by matching that text. Companion to
must/0019 (never silently discard model output) and must/0022 (never swallow an error): those
govern content that failed to parse and failures that were dropped; this one governs two halves of
the harness that quietly stop agreeing about the words.

When one site emits a string and another site detects it by matching a literal, the literal MUST
NOT be spelled out at both sites. Define it once as a named constant and reference that constant
from the producing site and from the matching site.

- The constant lives with the producer and is named for what it marks
  (`verify::SUBSTANCE_FLOOR_MARK`, `lint::ASSET_LINT_MARK`, `uitest::PAGE_GATE_PREFIX`).
- Rewording a marker means changing the constant, so both sides move together. Changing the
  wording on one side alone is the defect this policy exists to prevent, and lowercasing a shouted
  marker for should/0124 is the most common way to reintroduce it.
- The pairing needs a test that renders the real observation through the producing path and
  asserts the consuming matcher recognises it. A test that matches the constant against itself
  proves nothing; it must go through the code that builds the message.

Rationale: the failure is silent and it inverts a verdict. The consumer stops recognising the
producer's wording, so a failure is counted as a success and nothing anywhere records that the
match stopped working. Measured repeatedly before this rule landed (2026-08-01 sweep):
`metrics::step_obs_failed` matched "no step #", "already step" and "not a duplicate" long after the
producer had moved to "no step with id", "is already terminal" and "near-duplicate", so failed
steps were tallied as successes; a verify tip keyed on the substring "applied" and missed the
positional-edit wording "lines replaced"; a failure-streak counter watched only the
not-applied rejection and never saw the no-body and no-section rejections, so a worker livelock ran
undetected. Each was invisible until a human read the raw log, which is the same expensive silence
must/0019 measured for model output, one layer up.

Rationale for the shape of the fix: naming the string once removes the class rather than the
instance. Two literals cannot be kept in step by review, because the two sites are usually in
different files and the reviewer of one never reads the other.

Enforcement: code review at every new detection marker, plus a rendering test on the consuming
side. The pattern already in the tree is the marker constants in container/src/verify.rs and
container/src/lint.rs, consumed by the classifier in the same file, and the rendering tests
`prompts_use_plain_language_not_all_caps` and `tool_observations_use_plain_language`
(container/src/prompt.rs), which render real artifacts rather than fixtures. No scanner can tell a
deliberate literal from a drifted copy of a marker, so the mechanical half is the rendering test
and the rest is review.
