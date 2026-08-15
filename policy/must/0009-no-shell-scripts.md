# 0009 No shell scripts, no shell control flow — logic is Rust

Level: MUST
Scope: all tracked files in this repository. Extends must/0008 (Rust is
the implementation language; no Python) to shell scripting.

1. Shell scripts MUST NOT be checked into this repository. No `*.sh` files, and no tracked file
whose body is a shell script (an executable of shell logic beginning with `#!/bin/sh`,
`#!/bin/bash`, or `#!/usr/bin/env bash`). Host-side orchestration — build/deploy, container
bootstrap, dev-base provisioning, the offline-invariant check, the system tests — is implemented
in Rust as `lamalium host <sub>` (`container/src/host.rs`), not as shell scripts.

2. Shell control flow MUST NOT be composed in checked-in strings. Invoking a shell for a
single command is sometimes unavoidable and is allowed; composing logic in it is not. Any string
this repository ships that reaches a shell (`sh -c`, a broker exec, an nspawn/systemd-run command)
must be one command. Forbidden inside such strings: `&&`, `||`, `;` sequencing, `if`/`for`/
`while`/`until`/`case`, and `$( )` command substitution. Sequencing, branching, and loops are
written as Rust logic that issues one command per exec and branches on exit codes — see
`broker::exec_in_work` / `broker::exec_via_bus` (executor closures), `verify::run_full_gate_with`
(the 3-stage gate), `verify::revert_to_green` (the green-anchor revert), and
`supervisor::green_snapshot_with` (the snapshot) for the pattern.

What is still allowed (none of these composes shell logic):

- Invoking external TOOLS from Rust via `std::process::Command` — `cargo`, `podman`,
  `debootstrap`, `systemctl`, `machinectl`, `tar`, `install`, … Calling a program is not writing a
  shell script. Prefer direct argv (no shell at all) where no shell feature is needed.
- A single-command `sh -c "<one command>"` where a shell feature is genuinely required:
  quoting, a redirect (`2>&1`, `>/dev/null`), a builtin (`command -v a b c`), or one pipe as a
  data conduit (`… | tail -4`). The moment a second command joins via `&&`/`;`, it belongs in Rust.
- Agent `run` commands MAY use the container's POSIX tools (sh, awk, curl, …) as runtime glue
  (must/0008) — those are authored by agents at runtime, not checked-in code.
- Declarative config is not a script: `*.toml`, `*.network`, `*.service`, `Dockerfile`,
  `*.nspawn`, Markdown.

Rationale: one reviewable, testable, statically-typed implementation language (Rust) for all logic —
including host tooling and every harness-authored command sequence — instead of untested, brittle
shell that drifts and breaks silently. History: the six former `host/*.sh` were ported to
`lamalium host build|bootstrap|sync-roles|check-offline|provision-dev-base|systest`; the embedded
shell scripts (the full gate chain, the green-anchor revert, the green snapshot, the uitest
harness script, the bootstrap heredoc, the broker probes) were ported to Rust control flow on
2026-07-19.

Enforcement: two self-tests in `container/src/cli.rs`, run by `cargo test` = every full gate:

- `no_tracked_shell_script_files` — rejects tracked `*.sh` files and tracked files with an
  sh/bash-family shebang (`git ls-files` based).
- `no_shell_control_flow_in_source_strings` — extracts string literals from `container/src/*.rs`
  and rejects shell control-flow composition inside them (single commands, pipes, and redirects
  pass).

Both tests were deliberately made to fail once at introduction to prove they bite (should/0121).
