# 0103 Diagnose by isolation — one variable at a time

Level: SHOULD
Scope: anyone (human, claude-remote, an agent) debugging a failing behavior in this system —
especially OS/sandbox/systemd/cgroup/namespace failures, where "it started" rarely means "it worked".

When a thing fails or misbehaves, the diagnosis SHOULD proceed by isolation, not by guessing:

1. Change one variable at a time. Start from a known-good minimal configuration and add or
   remove a single factor per step until the culprit is identified. SHOULD NOT swap a whole set of
   options/flags/properties at once — that conflates causes and you learn nothing about which one
   mattered. Build a deterministic, minimal reproduction (e.g. a probe that exercises just the
   suspect) in preference to an end-to-end run with many moving parts and a slow/non-deterministic
   loop (an LLM step, a full task).

2. Default to "my config is wrong," not "the environment is broken." Most failures here have
   been self-inflicted (a cross-mount bind, a wiped path, a stale drop-in, an idle agent), not a
   platform limitation. Exhaust the isolation steps above before concluding something is unsupported.

Companions — the other halves of a sound diagnosis, each its own policy:
- confirm every step by its observed effect, never an exit code / "it started" — should/0116;
- consult the version-matched authoritative documentation before theorizing — should/0113;
- prove a suspected cause with a test or a direct observation, not reasoning alone — should/0110.

Rationale: every painful debugging episode in this project (the Phase-A slice that silently didn't
apply, the Phase-B `EXIT_NAMESPACE` failures, `PrivateNetwork` being a no-op under nspawn) was
prolonged by changing several things at once and/or trusting an exit code instead of the effect.
Isolation + effect-verification + version-matched docs would have found each in one pass.

Enforcement: convention + code review + the agent system prompts (which reference `policy/`).
Worked examples and the concrete gotchas live in [docs/design/LEARNINGS.md](#0700a4a8-8b19-41bb-a4ce-2dd5e407d084).
The `ctl probe` diagnostic exists precisely to do step 1 for `systemd-run` sandbox properties.
