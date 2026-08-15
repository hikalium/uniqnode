# 0106 Current state is a gauge; the event log records only changes

Level: SHOULD
Scope: anything that records or exposes the runtime state of agents, the supervisor, devices, tasks, or
the bus — status writers, the viewer/feed, `log_event`/`sup_log`, metrics endpoints, dashboards.

Keep two kinds of data strictly separate:

- Current state = a gauge. "What is true right now" (an agent's state, a device's up/down, a
  heartbeat, queue depth) is a value that is overwritten in place and polled on demand. It lives in
  a snapshot the reader pulls — `status.json`, the `/metrics` endpoint (Prometheus text), `/api/tree`.
  The UI must read current state from a gauge, never reconstruct it from the log.
- Event log = state changes + discrete events only. The append-only time-series (`agent.jsonl`,
  `supervisor.jsonl`, the feed) records transitions (idle→busy, became paused, finished, verified,
  device went up/down, an error) and one-shot events (a step, a steer, a bind) — never a per-tick
  snapshot of the current state.

Rationale: replaying current state into the append-only log every loop is pure noise that nothing consumes
and that grows without bound — it bloated a single `agent.jsonl` to ~30 MB of `idle`/`paused` lines,
slowing every read, parse and time-reconstruction, and burying the real transitions. A gauge is O(1) to
read and always current; a change-only log is small and is exactly the history you want.

Litmus test. Before emitting to the event log, ask: "Is this a change or a one-shot event, or am I
re-stating something already true on the previous tick?" If it's the latter, write/overwrite a gauge
instead (and expose it at `/metrics`). Before reading current state, ask: "Am I polling a gauge, or
scanning the log?" — it must be the gauge.

Concretely in this repo: agent daemons write `status.json` every tick (gauge) but `log_event` a state only
on transition; `relay_device_status` overwrites `device/status.json` (gauge) but `sup_log`s only when the
device state changes; the supervisor overwrites `bus/heartbeat` (gauge) each loop and never logs it;
`/metrics` exposes all current state for scraping; the viewer reads current state from `/api/tree`
(status.json) and uses the feed only for changes/events.

Complements should/0103 (verify the effect) and A9.11 (max-verbosity logging is for events, not for
re-stating state).

Enforcement: review at the emitting and the reading site, using the litmus test above; the split as
it already stands in this repo is listed in the paragraph before it.
