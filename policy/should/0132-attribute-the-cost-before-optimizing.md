# 0132 Attribute the cost to a mechanism before optimizing, and control the measurement's confounders

Level: SHOULD
Scope: any performance work — latency, throughput, resource use — in the harness, the viewer, or
host tooling. Companion to should/0110 (prove it, do not guess) and should/0116 (verify by the
observed effect), which require proof but do not say how to keep the proof honest.

Name the mechanism you believe is costly and measure it directly before writing the fix; then take
the primary evidence from a measurement whose confounders are controlled.

- Attribute first. A profile, an strace, a targeted bench, or a server-side A/B that isolates the
  suspected mechanism. Only after the cost is attributed does the fix get chosen, because the
  attribution frequently rules the expected fix out.
- Control contention. A measurement taken against a live system shares the machine with whatever
  else is running; when its spread is wider than the effect you are chasing, it cannot be the
  primary evidence. Use it as corroboration and take the primary number from the isolated path.
- Control cache warmth. Back-to-back samples re-read what the previous sample just warmed. When
  the situation being modelled is cold to the user, space the samples so it is cold in the
  measurement too, and say which regime a number belongs to.
- Say which measurement the claim rests on, so a reader can judge its reach (the same obligation
  should/0128 puts on absence claims).

Rationale (measured 2026-08-02, viewer feed latency): the plausible cause was response body size;
the real one was a per-entry `stat` inside directory enumeration, found by strace (5521 entries,
5521 stats) and removed by reading the directory entry type instead. Attribution also ruled out the
expected remedy — log segment compaction, which would have pushed against a documented invariant —
as unnecessary. Two confounders each produced a wrong number first: a real-browser probe against
the live tree had a 95 percent interval wider than 300ms from contention with running agents, so a
25 percent server-side improvement was undetectable in it; and continuous reloads re-read the same
segments within a few hundred milliseconds, leaving the page cache warm and halving the median
relative to what a person actually waits for.

Enforcement: convention; review of any performance change, which asks which measurement attributed
the cost and what was controlled. The measurement tools in the tree are the bench harness
(container/benches/metrics.rs) and the real-browser probe (container/src/loadprobe.rs), whose
sample interval exists precisely so the cold regime can be measured. The landed results are in
[docs/design/VIEWER.md](#23027dd3-61a9-41db-8476-42d82acf8208) and
[docs/analysis/20260802-viewer-endpoint-load.md](#6276a6eb-b4ac-46a5-8341-5d86c14d2f25).
