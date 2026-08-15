# 0117 Mechanize every manual environment repair into the rebuild path

Level: SHOULD
Scope: any hand-executed fix to an environment that is (re)created by automation — provisioning,
bootstrap, image builds.

When an environment defect is repaired by hand, the repair is not finished until it also exists
as an idempotent, executable operation wired into every path that (re)creates the
environment. If the rebuild path can reintroduce the defect, the rebuild path must also remove
it. Documentation, memory, or a warning comment is not a countermeasure — the next rebuild does
not read warnings. Corollary: prefer diagnosing why the defect appeared over patching the
symptom; the mechanized repair should address the producing mechanism.

Rationale (measured 2026-07): a hand-removed lock file reappeared because the rebuild procedure
itself recreated the damage on every re-run; worse, the prior "fix" recorded in docs was the
causal mechanism.
Enforcement: review of the repair — the reviewer asks which rebuild path now performs it, and
whether re-running that path from scratch still produces the defect. A repair that exists only as
prose does not satisfy this policy.
