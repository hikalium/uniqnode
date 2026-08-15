# 0008 No Python — Rust is the implementation language (inside and outside the container)

Level: MUST
Scope: all executable code in this project — the agent runtime / supervisor / CLI, host tooling,
and any artifact an agent produces or runs — both inside the container and outside on the
host. Companion: should/0101 (std-first dependency discipline within Rust).

Python MUST NOT be used anywhere in lamalium — not in the system binary, not in host scripts,
not in `ctl probe`/diagnostics, and not as an agent deliverable or an agent's `run` command.
This holds in both worlds: the container runtime and host-side tooling. No `python`/`python3`,
no `.py` files, no `python -c`, no `-m http.server`. The mere presence of `python3` in the
container image (it ships in the base) is not a license to call it.

Rust is the language. Executable logic MUST be Rust — shipped as the single static `lamalium`
binary (musl, dependency-free in the container) or, for agent deliverables, as Rust source/binaries.
A web server, a data tool, a log viewer, etc. are written in Rust (the standard library alone covers
most needs — e.g. `std::net::TcpListener` for HTTP — so most tools add zero crates and stay
offline-buildable).

Accepted exceptions (NOT deviations):
- Host orchestration is Rust (`lamalium host …`, `container/src/host.rs`) — build/deploy,
  bootstrap, dev-base provisioning, offline checks, system tests. It MAY invoke host CLI tools
  (`apt`/`debootstrap`/`machinectl`/`systemctl`/`curl`/`podman`/`tar`) from Rust via `Command`; that
  is tool invocation, not "implementation in another language". Checked-in shell scripts are
  forbidden (must/0009).
- Agent shell `run` commands MAY use the container's POSIX tools (sh, awk, curl, …) as glue, but
  MUST NOT invoke a Python interpreter, and any non-trivial program an agent builds MUST be Rust.
- Declarative config / docs — `*.toml`, `*.network`, `*.service`, Markdown — not implementation.

Dependency fetching for Rust (cargo/rustup needs the network, but the box is offline by default,
should/0102) MUST go through a scoped, time-boxed path — `cargo vendor` for reproducible offline
builds, and/or an allowlist proxy opened only for the duration of a dedicated fetch tool — never a
standing general egress grant. (See DESIGN §A10 `net:proxy` / dev-base toolchain.)

Rationale: one memory-safe, dependency-free, statically-linked language keeps the container minimal
and auditable, makes every running program reviewable in one toolchain, and avoids an interpreter as
an ambient capability an agent could lean on. "There happens to be a `python3` in `/usr/bin`" is
exactly the kind of implicit power this project removes.

Enforcement: code review + this policy (referenced by the agent system prompts) + a CI/`grep` guard
(reject `python`/`\.py\b`/`-m http.server` in tracked files and in agent `run` commands).
