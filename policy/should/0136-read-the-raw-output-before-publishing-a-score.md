# 0136 A per-subject score is not published until the low scorers' raw output has been read

Level: SHOULD
Scope: every measurement that ranks subjects the harness does not control — model bake-offs
(`ctl bench`), backend comparisons, A/B series over prompts or conditions, any table that will be
read as "X is better than Y". Sibling of should/0123 (suspect the harness before the model): that
policy governs diagnosis of one misbehaving agent, this one governs publication of a comparison.

A score measures the whole path — subject, prompt, parser, backend, scheduler, grader — and is
attributed to the subject only after the rest of the path has been excluded. Before a per-subject
number is reported to anyone, the raw output behind the LOW scores SHOULD be read.

Concretely, before publishing:

- Read the actual generated text of the worst-scoring subjects, not the aggregate. The failure
  shape names its own cause: a refusal, a wrong value, a protocol slip, or garbage.
- Carry a signal that separates "measured it" from "guessed it" (`reach_shell`), and a signal
  that counts harness-side rejection (`parse_failures`). A subject whose rejection count is high
  is a claim about the harness, not about the subject, until proven otherwise.
- Check that every capability the task needs is actually granted for the trial. Effective tools
  are role ∩ grants (must/0006); a missing grant scores every subject zero and looks like a hard
  task.
- Split the run by anything shared between subjects — backend host, machine, time window. A score
  that differs by host is measuring the host.
- Keep the produced artefact in the result row, not just the verdict. A grading rule that turns
  out to be wrong is then re-run over stored output instead of over the fleet (must/0019 applied
  to measurement).
- State the denominator and the interval. At n=10 a 10/10 and a 7/10 do not differ.

Rationale (measured 2026-08-08, the `chat-cpu-count` sweep over 31 models). Eight defects were
found while reading raw output that the aggregate had already scored as model quality: calls the
model wrote inside its reasoning block executed as actions; a fence after a body-less tool
discarding the round (314 steps); `say` bodies deleted for arriving under a neighbouring key
(77 rejections, 15 of them holding the correct answer); a fence holding several calls read as a
body; the fleet liveness probe declaring every openai-protocol backend offline; model residency
filling VRAM until generation degenerated into repeated `?`; and the grader itself reading a
sentence-final period as a decimal point (19 correct answers failed). Two subjects moved from
0/10 to 75% and 3/10 to 80% once these were fixed. Every one of them would have been published
as a difference between models.

Enforcement: convention at publication time. Mechanically, a measurement harness that ranks
subjects carries the separating signals in its result rows and its report
([docs/design/BENCH.md](#6d857ddb-941f-4337-a996-e746cb66ed74) §4, §6) — a report that shows only
a pass rate cannot be audited by its reader.
