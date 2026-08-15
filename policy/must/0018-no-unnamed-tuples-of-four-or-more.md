# 0018 Unnamed tuples stop at three elements — name the members beyond that

Level: MUST
Scope: all Rust source in this repository (`container/src/`, `container/tests/`,
`container/build.rs`) — tuple types, tuple expressions, tuple-struct declarations, and enum
tuple variants. Companion to must/0015
(no unexplained abbreviations): 0015 governs the words in a name, this governs values that have
no name at all.

A tuple (or tuple-struct / enum tuple variant) with four or more unnamed elements MUST NOT
be used. Use a struct (or an enum variant with named fields) so every member's meaning is
explicit in the source. Three or fewer positional elements remain allowed — pairs and triples
read fine; beyond that, a reader must reverse-engineer positions (`e.4`, `|(_, _, x, _, y)|`)
and every insertion silently renumbers the members.

- Applies to type positions (`Vec<(A, B, C, D)>`), expressions (`(a, b, c, d)`), destructuring
  patterns, `struct Name(A, B, C, D);`, and `enum E { V(A, B, C, D) }`.
- The fix is a named type next to the code (`struct FeedLine { ms, id, kind, bad, text, task }`)
  — not a comment explaining the positions.
- No grandfathering: existing violations were converted when this policy landed.

Rationale: the 2026-07-24 feed rework grew an anonymous 5-tuple into a 6-tuple
(`(i64, String, &'static str, bool, String, String)`); every call site and test had to be
re-read to learn which String was which. Positional meaning is jargon at the value level —
the same tax must/0015 bans for identifiers.

Enforcement: `cargo test` self-test `no_unnamed_tuples_of_four_or_more` (container/src/hygiene.rs)
scans every `.rs` file — comments and string literals stripped — and fails the build on any wide
unnamed tuple, tuple struct, or tuple variant. Runs as part of the standard gate; review covers
what the scanner's heuristics cannot see (e.g. shift operators confusing the angle-bracket
tracking).
