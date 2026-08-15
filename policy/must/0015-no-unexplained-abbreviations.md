# 0015 Do not abbreviate names without a clear reason

Level: MUST
Scope: all names a reader must decode — variables, functions, types, modules, file names,
CLI subcommands, config keys — in code, docs, and tooling. Companion to must/0012 (no opaque
jargon): 0012 governs prose, this governs identifiers.

Use the full word in a name. Coining an abbreviation (truncation, vowel-dropping, in-house
initialism) is forbidden unless there is a clear, stated reason — an abbreviation makes the
reader decode the name back into the concept, which lowers readability for every future read
to save the writer a few keystrokes once.

- Allowed without justification: abbreviations that are themselves the standard term a reader
  already knows — `id`, `max`, `min`, `len`, `URL`, `HTTP`, `LLM`, `TOML` — and idiomatic
  tight-scope locals (a loop index `i`, a short closure parameter).
- Everything else: write it out. `supervisor`, not `sup`; `request`, not `req`; `configuration`
  or the standard `config`, not `cfg`.
- A clear reason (e.g. a hard length limit imposed from outside) MUST be recorded where the
  name is introduced.
- Recorded binding example: `supervisor` must not be shortened to `sup` (rejected in the
  main.rs decomposition plan, 2026-07-19).
- Existing names are grandfathered but MUST be renamed to the full word when the surrounding
  code is already being restructured (e.g. moved to a new module).

Rationale: the user found `sup_*` module names unreadable. A name is read far more often than
it is written; unexplained abbreviations are jargon at the identifier level and tax every
reader. The specific ban is kept as the measured precedent of the general rule.

Enforcement: convention; review. Applies to the main.rs decomposition and all new code.
