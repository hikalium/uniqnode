# 0119 Reference items by stable identifier, never by position or by an unresolvable description

Level: SHOULD
Scope: cross-references in documents, comments, and data, and the references a message sent to the
operator or to another session makes — anywhere one artifact or message points into
another.

Refer to items by stable identifiers — names, titles, dedicated IDs — never by position
(item numbers, section numbers, line numbers): positions silently change when content is edited,
and every positional reference elsewhere rots. When ordering itself encodes meaning (e.g.
priority), do not add index numbers on top of it: they duplicate what order already expresses
and drift independently of it.

A description that only the author can resolve is a position in disguise. "The other thread",
"the item above" and "the earlier request" point at a target through where the reader is assumed
to stand, and a reader who arrives from somewhere else, or later, cannot tell which target is meant.
A message that points at another thread, issue, pull request, document or command SHOULD name the
target by its title, and SHOULD carry a link to it wherever the medium renders one (a thread link
in a project thread, a URL elsewhere). An author who does not hold the identifier yet, because the
target is still being created, obtains it before mentioning the target, and leaves the target out
of the message until then.

Rationale (measured 2026-07): roadmap item numbers were referenced from code and sibling
documents; every insertion or completed-item deletion invalidated them, while order already
carried the priority.
Enforcement: for documents the rule is mechanical — must/0013's UUID anchors are its instance, and
`lamalium host check-docs` fails the deploy on an unresolved anchor or a path-based doc link. Data
and prose outside docs/, and messages, are covered by review.
