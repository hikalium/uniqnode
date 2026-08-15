# 0022 Never swallow a Result's error

Level: MUST NOT
Scope: all Rust source in this repository (`container/src/`, `container/tests/`,
`container/build.rs`). Companion to must/0019 (never silently discard model output): 0019 governs
content that failed to parse, this governs a failure the code was explicitly told about and dropped.

A `Result`'s error MUST NOT be discarded silently. The statement-level `EXPR.ok();` idiom does
exactly that: it turns the error into `None` and drops it on the floor, so the failure leaves no
trace in any log, gauge, or return value. Do one of three things instead:

- propagate it with `?` and let the caller decide;
- handle it at the site — branch on it, report it, fall back deliberately;
- if the failure is genuinely ignorable, log it with `best_effort("what", <result>)`, which records
  the failure instead of erasing it.

Binding the value (`let x = foo.ok();`) is a legitimate Result-to-Option conversion, not a swallow,
as long as the `None` case is then handled.

Rationale: a dropped error is indistinguishable from success. The system keeps running on a state
nobody chose, and the first visible symptom surfaces far from the cause — the same expensive silence
must/0019 measured for model output, one layer down. Six such sites existed when the check landed
and were converted with it.

Enforcement: `cargo test` self-test `no_swallowed_result_via_ok` (container/src/hygiene.rs) scans
every `.rs` file under container/ with comments and string literals blanked first, so a `.ok();`
inside a fixture string is never flagged; the scanner itself is pinned by
`swallowed_ok_discards_flags_bare_discards_not_bound_conversions`.
