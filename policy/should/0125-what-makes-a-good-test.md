# 0125 A good test checks a behavior, fails clearly, and never lies

Level: SHOULD
Scope: everyone who writes or reviews tests, human or agent: workers writing page tests
(work/uitest.js), the QA test generator, evaluators reviewing test quality, and unit tests in
this repository. Distilled from "Software Engineering at Google", ch. 11 (Testing Overview).

- Test behaviors, not implementations. Probe what a user or caller can observe: an element
  exists, an action produces its promised effect, the page loads without errors. Do not assert
  internal variable or function names. If a check must name an identifier (a selector, an id),
  that name becomes a contract; state it explicitly in the failure detail so the reader can
  either implement it or challenge it.
- A failing test must tell the repair story by itself. Every failure carries a message and a
  detail: the message states the violated expectation in one sentence; the detail names the
  exact probe that was made and what was actually found, gathered at failure time. A bare
  "not found" forces the reader to rediscover the test's intent before they can start fixing.
- Deterministic or absent. No dependence on wall-clock timing, execution order, or earlier
  runs; every wait polls a condition with a deadline. A test whose verdict flaps on an
  unchanged subject is worse than no test, because it teaches everyone to ignore red.
- Keep each check small and obvious. One behavior per check, minimal control flow, readable
  top to bottom. Tests are also documentation: a check's message should read like a
  specification sentence.
- Prefer many narrow checks over few broad ones. Focused checks localize the fault; a broad
  whole-journey check is a sanity net, not the primary bug detector.
- If you rely on a behavior, put a test on it. When you fix a bug, add the check that would
  have caught it (proven by restoring the defect and watching that check fail — should/0137).
  An untested behavior is a behavior someone will break unnoticed.
- Coverage is a floor, never a goal. Add a check to pin a behavior you need, not to move a
  number.

Rationale (measured 2026-07-20): a generated page test asserted an internal variable name and
reported failures without expected/actual details; two different repair agents then failed to
diagnose the mismatch for nine hours. The same information, stated in the failure detail,
would have made it a one-turn fix.

Enforcement: the mechanical floor of these principles is must/0017 (page-test contract);
the qualitative rest is review guidance for evaluators and authors.
