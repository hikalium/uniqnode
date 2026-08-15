# 0118 Make adding an instance a config edit, not a code change

Level: SHOULD
Scope: any resource class that can have multiple similar instances — servers, backends, devices,
replicas.

Define multi-instance structures by enumerating instances in configuration data. Adding the
Nth instance must be an entry in a config file — never new code, a new service unit, or a copy
of a per-instance artifact. The first instance sets the pattern: design for enumeration from the
start, and treat an artifact named after a specific instance as a design smell to fix before
shipping. Runtime should pick up config changes without a privileged redeploy where feasible.

Rationale (measured 2026-07): a second backend machine initially got its own dedicated unit
files; the config-enumerated redesign made every future machine a two-line edit.
Enforcement: review of the first instance, where the pattern is set. The visible symptom to look
for is an artifact named after one specific instance.
