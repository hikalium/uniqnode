# 0119 Reference items by stable identifier, never by position

Level: SHOULD
Scope: cross-references in documents, comments, and data — anywhere one artifact points into
another.

Refer to items by stable identifiers — names, titles, dedicated IDs — never by position
(item numbers, section numbers, line numbers): positions silently change when content is edited,
and every positional reference elsewhere rots. When ordering itself encodes meaning (e.g.
priority), do not add index numbers on top of it: they duplicate what order already expresses
and drift independently of it.

Rationale (measured 2026-07): roadmap item numbers were referenced from code and sibling
documents; every insertion or completed-item deletion invalidated them, while order already
carried the priority.
Enforcement: for documents the rule is mechanical — must/0013's UUID anchors are its instance, and
`lamalium host check-docs` fails the deploy on an unresolved anchor or a path-based doc link. Data
and prose outside docs/ are covered by review.
