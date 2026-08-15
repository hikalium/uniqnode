# 0116 Verify a configuration change by its observed effect

Level: SHOULD
Scope: any change to configuration — service definitions, environment variables, runtime/kernel
settings, config files.

A configuration change is complete when its intended effect is observed in the running
system, not when the edit is made. Before applying, decide how the effect will be observed.
After applying, verify through that observation — the system's effective state (a live config
dump, resource usage, measured behavior), never the file you edited or the exit code of the
apply step. Editors fail to save, daemons ignore unknown keys, fallbacks mask failures —
silently. A change whose effect was not observed is unverified and must not be reported as done.

This is the canonical effect-verification rule: the same standard governs observations made while
diagnosing (should/0103) or while proving a claimed fix (should/0110) — an exit code or "it
started" is never the effect.

Rationale (measured 2026-07): two consecutive incidents in one day — an edited unit file that
was never saved, and a service that started successfully while silently running on a fallback
compute path at 1/8 speed.
Enforcement: review of any change that touches configuration — the reviewer asks which observation
showed the effect. A change reported as done without naming that observation is not done.
