# 0020 Parse only the format the prompt actually asked for

Level: MUST
Scope: every harness site that machine-reads LLM output, paired with the prompt that requested
that output (tool protocols, judge/vision/witness questions, structured answer contracts).

A parser's anchors and discriminators MUST be limited to tokens and structure the prompt
explicitly requested. Betting on a model's stylistic habits — markdown bullets, capitalised
emphasis, JSON it was never asked for, a particular quoting style — as the thing the parser
keys on is a policy violation. Two corollaries:

- No format-convention field multiplexing. Heterogeneous fields (a defect list and a page
  description; an answer and a rationale) must not share one free-form string with a stylistic
  convention as the separator. Separate them structurally: harness-specified anchor tokens
  (`content:`, `page:`), separate questions, or separate calls.
- Fixtures follow the contract, not the habit. A test fixture written in a format the
  prompt does not require pins the bet, so the suite stays green while the real model's
  conforming answer is destroyed.

Rationale: the looks_block incident (2026-07-30). The vision prompt asked for defects "one
short line each" — no bullets — plus a trailing sentence describing the page. The consumer
discriminated the two fields by a `- ` prefix that only the test fixture used. gemma answered
the prompt literally ("blank page", no bullet) and the parser classified every line as
description and dropped it all; the decision-point block regressed to exactly the burial the
2026-07-21 operator ruling had built it to fix. The model was conforming; the parser was
enforcing a contract that existed only in the fixture. Related: must/0019 covers what to do
when parsing fails; this policy removes the class of parsers that fail on conforming input.

Enforcement: code review — when reviewing a parser of model output, read the prompt it is
paired with and check every anchor appears there. Tests must include a conforming-but-unstyled
input case (the contract satisfied with none of the optional styling).
