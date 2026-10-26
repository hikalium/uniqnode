# 0141 Text a program emits at run time is written in English, in ASCII

Level: SHOULD
Scope: the wording a uniqnode program puts in front of a reader while it runs, when that reader is
a terminal, a log or a model. That covers what a command or a service writes to standard output,
to standard error or to a log (the journal and the log files of apply and autodeploy included),
an error message that reaches one of those, an HTTP response body that a program or an agent
reads, and the tool descriptions and tool results the MCP server gives a model. The test is that
the text is the program's own wording and that the text is read at run time rather than in the
source. Four kinds of text are outside this policy. The first is text the program only passes
through, such as a file name, a stored document or chunk, a search query or a model's answer. The
second is text that stands in for another party's words, such as a phrase a person is expected to
say. The third is the viewer page a browser shows to a person, with the messages it displays,
whose language is chosen with the page. The fourth is a message inside test code. The language of
documents and comments stays with should/0107. This is the same rule as lamalium's
should/text-a-program-emits-is-english-ascii (uuid:9b8d9c9b-b173-4ac1-b81b-4ec78e94679b).

The program's own wording SHOULD be English, written in printable ASCII with line feed and tab.
Text passed through is shown as it is, and a message that shows such text carries it in a
placeholder rather than in the literal. A program whose job is to judge text in another language
may name words of that language in its messages, and such a deviation is noted in a comment
beside the message or in the list of exemptions the source scan reads.

Rationale: an agent reads a command's output and an MCP tool's result as a tool observation, so
that text becomes part of a model's prompt and reads best as the plain English the model was
trained on. A person reads the same output in a terminal or in the journal and searches it with
grep, where an English word is typed without an input method. The surrounding tools (cargo, git,
systemd) already write English, so one language keeps a log readable as a whole. The cost is that
run-time text differs in language from the Japanese documents and comments, so a message first
thought of in Japanese is translated when it is written into the program.

Enforcement: the test `run_time_text_is_ascii` in node/tests/repo_hygiene.rs scans the string
literals of non-test sources under node/src and fails on a character outside the set above,
unless the line carries the exemption comment or the file is on the test's list of exemptions
with a reason. A message whose non-ASCII part arrives through a placeholder passes the scan, and
whether that part is really passed-through text is left to review. The Lean version of this
policy is the file of the same name with the extension .lean.
