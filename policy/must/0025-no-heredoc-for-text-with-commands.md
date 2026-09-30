# 0025 Never pass text that contains commands through a shell heredoc

Level: MUST
Scope: every edit made by humans, assistants, or agents to a file that carries executable
command blocks meant for someone else to paste — above all the operator procedures in
docs/mop/ and the operator steps in docs/plan/ that contain `sudo … bash -s` blocks — and every
instruction written for a subagent that will make such edits.

Such a file MUST be changed only through the editor's file tools (Edit / Write, or an editor),
or by a program that reads the file from disk and writes it back. Its content, in whole or in
part, MUST NOT be passed through a shell heredoc (`<<EOF`, `<<'PYEOF'`, …), `echo`, or any
other route where the shell parses the text; the same holds for a script that embeds that text.
A script that edits such a file is itself first written to a file with the file tools, then
run. An instruction handed to a subagent that may edit such a file MUST state this rule.

Rationale: a heredoc ends at the first line that equals its delimiter. The document's own
command blocks contain lines such as `EOF`, so a heredoc can close in the middle of the text,
and the rest of the document is then executed as shell commands in the author's shell. On vega
`sudo` needs no password, so an operator block written for a human to paste runs as root at
once. Measured instance 2026-10-01 05:00:19: while revising docs/mop/SYSTEMD.md, a subagent
passed a python editing script through `<<EOF`; the heredoc closed early and the legacy-unit
rollback block ran as root (`sudo bash -s`, visible in the sudo journal). It stopped only
because its first line found no archive under /var/tmp and `set -euo pipefail` ended it; the
units, `/etc/systemd/system` and the firewall were checked unchanged afterwards. The next such
block would not be so lucky: the removal block disables production units. Operator ruling
2026-09-30T23:40:15Z (relayed by the lamalium hub): adopt as MUST, keep the text of b22ce57, and
state the rule in every session's instructions to subagents.

Enforcement: convention, plus the standing instructions given to subagents.
