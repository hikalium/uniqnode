# 0012 Name the concrete behaviour — no jargon the reader must decode

Level: MUST
Scope: communication with the user, and in-tree docs/comments. Companion to should/0107.

Describe what actually happens in plain, concrete terms. Do not use an opaque word — jargon,
in-house shorthand, a metaphor — that the reader has to decode to recover the behaviour, when the
behaviour can be named directly.

- Test before using a term: would a reader who does not already know the internals understand it?
  If not, describe the behaviour instead. A precise standard term used exactly (e.g. "thrashing")
  is fine; a vague catch-all is not.
- Recorded binding example: do not use the word "churn". Name the behaviour instead:
  - 「空回り」（busy but making no progress）
  - 「無駄な再束縛の繰り返し」 / "repeated failed re-binding"
  - the standard CS term "thrashing" when that is precise.

Rationale: the user found "churn" unclear. Jargon that needs decoding slows the reader and hides
meaning; the specific ban is kept as the measured precedent of the general rule.
Enforcement: review. The recorded ban on the word churn is greppable in any tracked file; the general
rule — would a reader who does not know the internals recover the behaviour from this word? — stays
reviewer territory, and applies to diagnoses as well as descriptions (should/0123).
