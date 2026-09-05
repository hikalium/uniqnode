# policy/ — project policies, classified by RFC 2119 level

Policies for the uniqnode project are recorded here as small, individually-numbered Markdown
files, grouped into directories by their enforcement level. The levels use the requirement
keywords of [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) (as clarified by RFC 8174).

## Levels (= directories)

| Dir | RFC 2119 keywords | Meaning | Deviation |
|---|---|---|---|
| `must/` | MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT | Absolute requirements / prohibitions. Non-negotiable invariants of the project. | None. A violation is a defect. |
| `should/` | SHOULD, SHOULD NOT, RECOMMENDED, NOT RECOMMENDED | Strong defaults. Followed unless there is a specific, recorded reason. | Allowed only with an explicit justification noted in the deviating artifact (e.g. a comment or commit message). |
| `may/` | MAY, OPTIONAL | Permitted choices. Capabilities the project may use; neither required nor forbidden. | Free choice. |

## Provenance: seeded from lamalium

This corpus was seeded on 2026-08-15 from the lamalium project's policy corpus (lamalium commit
ca1fdca), copying only the policies that are generic engineering rules rather than rules about
lamalium's agent-container architecture. Consequences of that seeding:

- Numbering is shared with lamalium: the same rule has the same number in both projects.
  Gaps in the sequence (e.g. must/0000–0007, must/0016–0017) are lamalium-specific policies that
  were not adopted here — they are not retired uniqnode policies.
- New uniqnode-specific policies continue from the next free number in each band and must not
  reuse a gap number, so the shared numbers stay unambiguous across the two projects.
- The copied files are verbatim. Their Rationale sections cite incidents measured in lamalium,
  and their Enforcement sections may cite lamalium tooling (`lamalium host check-docs`,
  `cargo test` self-tests in `container/src/`). Read those citations as precedents from the
  originating project: they document why the rule exists and prove it is mechanizable, not that
  the mechanism exists in this tree. Here the corpus check (file format, retired stubs, every
  policy reference from SPEC/docs/source resolving to a live number) and the docs check (UUID
  anchors defined, unique and resolvable; link text naming the document it points at; no bold
  emphasis) are mechanized in `node/tests/repo_hygiene.rs` and run with every `cargo test`. The
  remaining policies are enforced by convention + review.

## File format

Each policy file:

```
# <id> <short title>

Level: <RFC 2119 keyword>
Scope: <what/who it applies to>

<the normative statement, using the keyword>

Rationale: <why>
Enforcement: <how it is enforced / where in code, or "convention">
```

Numbering: `must/` = `0000+`, `should/` = `0101+`, `may/` = `0201+`. Numbers are stable and are
never renumbered or reused.

Retirement. A policy that no longer applies keeps its number and becomes
`policy/<level>/NNNN-obsoleted.md`. Its entire content is the line

```
This policy is obsoleted. Remove reference to this policy.
```

followed by nothing but the ids of the policies the rule moved into, one per line (none if it was
withdrawn outright). The old body is deleted. This stub is the single exception to must/0010's rule
that superseded text is removed from the tree rather than marked.

## Operation

- Authority to edit. `policy/` is edited at the top level (the human operator; assistants only
  with the operator's review). Changes to `must/` always require a human decision.
- Precedence. `must/` overrides `should/` overrides `may/`. If two policies conflict, the
  stronger level wins; conflicts within a level are resolved by a human.
- Binding on whom. Policies bind every component of this project: the Rust code, the tooling,
  the documents, and human or assistant contributors.
- Enforcement, not just words. Wherever feasible a `must/` is enforced mechanically (tests,
  checks in the build gate). The file then cites where. Copied policies cite lamalium's
  mechanisms as precedents (see Provenance above).
- Change control. Every policy change is a git commit; the history is the audit trail.
