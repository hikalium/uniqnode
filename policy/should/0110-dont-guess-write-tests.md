# 0110 Don't guess — verify with a test or a direct observation

Level: SHOULD
Scope: all work — agent deliverables, the harness, and debugging. Companion to should/0103 (diagnose by
isolation) and should/0105 (LLM errors are usually missing context).

When you do not know why something behaves as it does, or whether a change actually works, do not assert a
cause or declare a fix from reasoning alone. Prove it.

- For logic, write a test. Pull the behaviour into a small pure function and add a `#[test]` that would
  fail without your fix and passes with it. Run it before claiming the fix works. Prove the "would fail"
  half by restoring the defect and watching the new test fail (should/0137).
- When a unit test does not fit, make a direct observation: add a temporary probe/log or an assertion and
  read the actual value, rather than inferring it. Then act on what you saw.
- Verify the effect, not a proxy: a green build / exit 0 is not proof the behaviour is right
  (the canonical effect-verification rule is should/0116).

A guessed root cause that "sounds right" burns cycles and ships bugs. Real case: a URL query-parameter
parse looked obviously correct, so its failure was blamed on stale binaries and reload races for several
rounds — the query was actually stripped one layer upstream, and a five-line unit test would have shown it
on the first try. When you catch yourself reasoning in circles about "why is it still X", stop and write
the test or print the value.

Rationale: empirical checks are faster and far more reliable than speculation, and a test left behind
guards the behaviour forever.
Enforcement: self-review — before asserting a cause or a fix, ask "what did I actually run that
proves this?"; if the answer is "nothing", write the test first.
