# 0137 A test added with a fix is proven by restoring the defect and watching the test fail

Level: SHOULD
Scope: every change that reports "a test was added" or "a test was strengthened" — Rust tests in
this repository, agent-produced tests, and the review of either. Sibling of should/0125 (what makes
a good test): that policy governs the properties of the test's text — behaviour-level, clear
failure, deterministic, honest; this one governs the experiment that proves this particular test
bites this particular defect. It is also the missing run in should/0110, which asks for a test that
"would fail without your fix", and the generalization of should/0121's "make the gate fail once on
purpose" from shell gate chains to unit tests. Distinct from should/0138 (probe with the request the
production caller builds): that policy governs the request a diagnosis sends into a running service,
where the subject is live and the code is the probe; this one governs the code a test is run
against, where the subject is the tree and the experiment is a revert.

"This test would have caught it" is a prediction. Before a repair is reported as done, put the
production code back into the shape that carried the defect, run the new test against it, and read
the failure. Carry the result with the fix: which line was restored, and which assertion went red.
Stated as a rule: a test added with a fix SHOULD be run once against the code as it stood before the
fix, and the result of that run SHOULD travel with the repair.

Concretely, when a fix adds a test:

- Restore the defect in the narrowest way that reproduces it — put back the old accessor, restore
  the deleted line, remove the call you added — then run the new test against that tree and read
  the failure message. Put the fix back and confirm green again.
- Seed every value the assertion reads so that the defective code and the repaired code would write
  different answers. Checking a counter for "0" when 0 is also the `unwrap_or` default asserts
  nothing, because both sides write it; if the behaviour under test really is "the counter returns
  to its default", start it from a non-default value so the return is what the assertion sees.
- Build the fixture that lets the restored defect reach the assertion. A guard that runs only for
  loop Rust tasks needs a loop Rust task fixture (`testsupport::loop_rust_fixture`); through an
  ordinary fixture the entire guard can be deleted with the suite still green.
- Enter through the production entry point and observe what the harness actually emits
  (`testsupport::FakeBus` for bus-mediated execution), so that deleting a call site turns the test
  red. A test that calls the inner function directly survives the removal of its only caller.
- Write expected values as literals. Deriving them from the same helper the test guards moves both
  sides together when that helper breaks, so the restored defect stays green.
- Repeat the experiment once per mechanism the finding names. One restored defect proves one
  mechanism; a finding that names four bookkeeping paths needs four runs, or the unproven ones ship
  under the word "fixed".
- Record the experiment where the repair is reported — commit message, review verdict, task note —
  in the form "reverted X, test Y failed with Z". Leave the reason beside any fixture whose shape
  exists only to make that failure reachable.
- Treat a test that stays green against the restored defect as an unfinished repair: strengthen the
  assertion or the fixture until it fails, and report the test as added only then.

Rationale (measured 2026-08-10, while repairing the findings of
[docs/analysis/20260810-source-audit.md](#8b90b655-c4df-4864-a99d-76184e659c44)). Three consecutive
repairs were stopped in review, each time because the reviewer broke the production code and the
newly added tests stayed green. Six tests were measured this way. An `edits_since_build` assertion
of "0" held against the defective code, because the counter is rewritten unconditionally from an
`unwrap_or(0)` default and the fixture started it at 0. Setting `let full_gate = false` — running
no fmt, no clippy and no tests at all — left the whole suite green, because the fixture was not a
loop Rust task. Deleting the entire `apply_agent_green` call left it green too: the one call site
had no test. Reverting every consumer of `broker::command_of` outside broker.rs (in agent.rs,
prompt.rs, metrics.rs and absteer.rs) to the `as_str()` read that caused the defect left it green.
A trial-wipe test asserted `is_dir()` on a directory the wipe never removes, so the fixture decided
the verdict; another asserted the names in a directory the test itself deleted before the wipe and
re-seeded afterwards. The tree carried 629 `#[test]` functions before the repair and none of them
moved. Two of the five bookkeeping paths the audit had named in §1.5 (the full gate,
`build_broken`, the green snapshot, `note_red_build`, `edits_since_build`) were thus left with no
guard at all; without these runs the only remaining trace would have been a document saying they
were fixed, and the next person to break them would have seen green.

Filed as should/ rather than must/ because some tests guard behaviour that never had a broken
shape — the first test of a new feature has no defect to restore. There the deviation is recorded
at the test: name what was varied instead (a value flipped, a branch removed), and say that the
variation was run.

Enforcement: convention at repair and review time — the evidence is a run that leaves no artifact,
so it lives in the commit message and the review verdict. must/0014 already names the half no
runner can see: a suite that stays green while exercising the retired contract. The nearest
mechanical relative is `test_count_regression` in container/src/verify.rs, which forces red when a
tree loses tests — it counts tests without asking whether any of them bite. The mechanization worth
building is a runner that reverts one hunk of a commit at a time, re-runs the tests that commit
added, and reports every hunk no test objects to. Until it exists, the worked instances to copy are
`loop_rust_fixture` and the fake bus in container/src/testsupport.rs, whose doc comments record
which weaker fixture stays green, and the tests
a_cargo_build_written_as_an_argv_list_is_still_a_build,
a_loop_rust_task_gets_its_own_verdict_from_the_full_gate and
a_green_gate_commits_the_tree_and_moves_the_revert_anchor in container/src/agent.rs.
