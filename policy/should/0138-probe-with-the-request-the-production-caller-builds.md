# 0138 Probe a running service with the request its production caller builds

Level: SHOULD
Scope: every hand-issued request used to judge whether a running service is healthy — `curl`
against an LLM backend, a one-off snippet against the chat surface or a daemon — and every
conclusion, document, retraction or operator request derived from one. Sibling of should/0103
(diagnose by isolation): that policy gives the method, one variable at a time, this one names the
baseline the isolation starts from — the payload the production caller actually sends. Sibling of
should/0126 (read logs through the command): that governs reading a channel through its one
reader, this governs writing a request through its one builder. Neighbour of should/0135 (one
judgement, one implementation): that policy governs a second implementation that lives in the
repository and is reviewed; this one governs the second implementation nobody reviews, typed into a
terminal and thrown away after it has already produced a conclusion. Distinct from should/0137
(prove a new test by restoring the defect): that policy proves a test by putting the defect back
and reading the failure; this one proves a symptom by sending the request the fleet sends. Both
refuse evidence gathered off the production path, one at build time and one against a live service.

A diagnostic request measures the request as much as the service. Every parameter the production
caller sends and the probe omits is a variable the probe introduced, and an omitted key is still
a value — the server picks it. So a probe SHOULD be issued through the code the fleet itself uses
to build the request, and a finding SHOULD name the builder it went through.

Concretely, when diagnosing a live service:

- Send the request through the fleet's own builder: `llm::llm_payload_for` for a completion,
  `llm::stream_payload_for` for a streaming round, `chat::say_ops` for a chat turn. Reach first
  for the instruments that already share them — `lamalium host prompt-probe` speaks through
  `stream_payload_for`, `lamalium ctl chat` posts the op list the WebUI posts.
- Open the builder and copy every field when a hand-written request is unavoidable. On the ollama
  path today that is `think`, `keep_alive` and `options.num_ctx`; on the openai path, `max_tokens`
  at the reasoning-tax multiple. Diff your body against the builder field by field before sending.
- Take each value from the source the fleet takes it from — `num_ctx` from
  `llm::resident_context_tokens` (bench: `bench_context_tokens`; seats: `role.num_ctx`) — rather
  than a number you chose for the probe, which measures a window nothing serves.
- Reproduce the symptom once through the production caller (a seat round, a bench trial, a chat
  turn) before recording it as a fleet failure. A symptom only the hand-written request shows is
  a finding about the request.
- Put that reproduction ahead of every expensive consequence: ask the operator for sudo, request a
  rollback, publish a retraction or file an incident only after the production path has shown the
  same failure.
- List in the conclusion each field you could not copy, and read the result as a claim about
  those fields too. "I omitted `num_ctx`" belongs in the finding, beside the symptom.
- Land a probe that did find a real defect as an instrument sharing the builder, the way
  `promptprobe` shares `stream_payload_for` (should/0135), so the next diagnosis inherits the
  fields.

Rationale (measured 2026-08-10, the orion Vulkan switch). Two hand-written curl requests produced
two false reports of a broken fleet in one day. The first omitted `num_ctx`. Vulkan reports
87.2 GiB free where ROCm reported 51.2 GiB, so ollama sized the window itself and started
llama-server with `-c 1048576 -np 4` (262144 cells per slot); `gemma4:31b` aborted one second
into load with `pre-allocated tensor (cache_k_l5) in a buffer (Vulkan0) that cannot run the
operation (NONE)` and HTTP 500. That was read as "Vulkan cannot host the fleet's main model": a
sudo rollback of a working configuration was asked of the operator, and a retraction of the
switch was committed to docs/design/OLLAMA.md (d529af9). No fleet caller can reach that abort —
`llm_payload_for` and `stream_payload_for` both send `options.num_ctx`, `bench_context_tokens`
and `role.num_ctx` supply it on the other paths, and a hygiene test fails the build on a
hardcoded window. Undoing the false retraction took two more commits (1fceac0, 17d918f). The
second curl omitted `think: false` and came back with an empty body, read as generation being
discarded; the model had spent 148 tokens on reasoning that ollama then dropped — the
reasoning-tax comment beside `llm_payload_for` already names that shape ("the same failure class
as gemma-without-think:false"). The two fields sit on adjacent lines of a builder that was never
opened.

Filed as should/ rather than must/ because nothing in the tree sees a curl typed into a terminal,
and a probe issued while a builder is itself under repair is a legitimate exception. Record which
fields the probe departed from and why, beside the finding.

Enforcement: convention at diagnosis time — a finding about a live service names the builder or
the caller its request came through, and the fields it could not copy. Mechanically the builders
are single and pinned (`llm_complete_openai_vs_ollama_payload` in container/src/llm.rs,
`llm_calls_take_their_context_window_from_the_residency_table` in container/src/hygiene.rs), and
the instruments that share them are the supported way to ask. The open gap is the hand-written
request: nothing prints the exact body a given model and seat would send, so a
`host llm-payload --model <m>` that dumps the builder's output for pasting would turn this rule
from a convention into an artifact a diagnostician copies.
