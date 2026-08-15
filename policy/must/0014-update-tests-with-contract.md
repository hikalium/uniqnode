# 0014 Update a contract's tests in the same change that alters the contract

Level: MUST
Scope: any change to an interface contract — protocols, data formats, API shapes,
validation/acceptance semantics.

When an interface contract changes, every test and fixture that exercises that contract MUST be
updated in the same change. Tests are part of the contract, not an afterthought: a suite left
failing (or silently passing while exercising the old contract) means the change is incomplete.
A red regression suite detects nothing — it must be repaired or the change reverted, never
ignored.

Rationale (measured 2026-07): a tightened input contract shipped without updating the
deterministic test suite; the suite sat red for two weeks and masked an unrelated scheduler
regression that landed during that window.
Enforcement: `cargo test` runs in every full gate, so a suite left red already blocks the deploy.
What review must add is the other half — a suite that stays green while still exercising the retired
contract, which no runner can detect.
