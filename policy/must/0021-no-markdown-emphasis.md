# 0021 No markdown emphasis — anywhere

Level: MUST
Scope: every string produced in this project — agent-facing prompts and tool observations,
policy files, design/plan documents, code comments, commit messages, viewer text, and chat
replies from assistants to the operator. All languages.

Markdown emphasis markers MUST NOT be used for emphasis in prose: no bold (`**...**`,
`__...__`) and no italics (`*...*`, `_..._`). If a clause needs more weight, give it a better
position (closer to the decision point) or its own sentence — not louder typography.

What is not emphasis and stays as it is:

- Structural markdown: headings (`#`), lists (`- `), tables, code fences and backticked spans.
- Literal asterisks and underscores with technical meaning: glob patterns (`tasks/**`,
  `*.sh`), code (`pub const PAGE`), regular expressions, identifiers with underscores.
- Quotations of pre-existing text that itself contains emphasis, when the quote must match
  the original byte-for-byte.

Rationale: emphasis typography is the same failure class as all-caps shouting (should/0124
records the measured harm in prompts: each marker over-biases its clause, and accumulated
markers cancel each other into pure noise). The same text is also read raw far more often
than rendered — agents receive documents as plain strings, terminals show literal asterisks,
and diffs and greps are cluttered by the markers. The policy corpus itself had accumulated
emphasis in 26 of 39 files before the 2026-07-30 cleanup (commit a247433, 170 lines changed
with zero content change), which shows convention alone drifts without a recorded rule.
Operator ruling 2026-07-30: this applies beyond prompts to every written surface, chat
output included.

Enforcement: mechanical. `lamalium host check-docs` (also step 0 of `host build`, so a violation
fails the deploy) flags prose emphasis in every tracked markdown file under docs/, policy/,
DESIGN.md, README.md, and roles/, allowlisting exactly the exceptions above (code fences and
backticked spans, unmatched glob/literal markers, intra-word underscores in identifiers). The
companion `lamalium host fix-emphasis` clears it by deleting only the `*`/`_` markers outside code
spans (idempotent; shares the gate's engine). Prompt artifacts stay covered by should/0124's
`prompts_use_plain_language_not_all_caps`.
