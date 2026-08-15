# 0101 Prefer the standard library — adding a dependency requires recorded justification

Level: SHOULD
Scope: all first-party Rust code (agent runtime, supervisor, CLI, host tooling) and agent-produced
Rust deliverables.

New functionality SHOULD be built on the Rust standard library alone. Adding a crate — or growing
the dependency tree of an existing one — SHOULD NOT happen without a recorded justification (in
the commit message or a code comment) naming what the crate provides that std cannot reasonably
cover. std goes further than habit suggests: `std::net::TcpListener` serves HTTP,
`std::process::Command` orchestrates external tools, `include_str!` embeds assets (should/0112);
most tools in this system ship with zero new crates.

Which language to use is NOT this policy's question — must/0008 (Rust, no Python) and must/0009
(no checked-in shell scripts) govern that as hard rules. This policy governs how much you pull in
once you are writing Rust.

Rationale: the container is offline and the single static binary is the whole toolchain — every
dependency enlarges the audit surface, the offline vendor set (should/0102), and the artifact
every review must cover. A small, boring dependency tree is a feature.
Enforcement: code review of `Cargo.toml` changes; `container/` is a single crate with a
deliberately short dependency list.
