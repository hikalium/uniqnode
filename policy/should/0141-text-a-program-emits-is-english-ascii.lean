/-
Lean version of policy/should/0141-text-a-program-emits-is-english-ascii.md. The Markdown file is
the source of the rule. It is the same rule as lamalium's
policy/should/text-a-program-emits-is-english-ascii.md (uuid 9b8d9c9b-b173-4ac1-b81b-4ec78e94679b),
and this file restates its level, its applicability test and its requirement as definitions that
type-check, so a reader can run a case through them.
-/

namespace Policy.P0141TextAProgramEmitsIsEnglishAscii

/-- The RFC 2119 levels. -/
inductive Level where
  | must
  | should
  | may
  deriving DecidableEq, Repr

def level : Level := .should

/-- Who reads the text while the program runs. -/
inductive Reader where
  /-- Standard output or standard error, read in a terminal. -/
  | terminal
  /-- A log, such as the journal or a log file. -/
  | log
  /-- A response body that a program or an agent reads. -/
  | programOrAgent
  /-- A prompt, a tool description or a tool result rendered for a model. -/
  | model
  /-- A page a browser shows to a person, with the messages that page displays. -/
  | personInBrowser
  deriving DecidableEq, Repr

/-- Where the wording comes from. -/
inductive Origin where
  /-- The program's own wording. -/
  | ownWording
  /-- A file name, the contents of a file, a stored document or a model's answer. -/
  | passedThrough
  /-- Another party's words, such as a simulated request or a phrase a person is expected to say. -/
  | standsInForAnotherParty
  deriving DecidableEq, Repr

structure RunTimeText where
  chars : List Char
  reader : Reader
  origin : Origin
  inTestCode : Bool
  /-- A deviation, such as judging text in another language, is noted with its reason. -/
  notedDeviation : Bool

/-- The characters the requirement admits: printable ASCII, line feed and tab. -/
def admitted (c : Char) : Bool :=
  (0x20 <= c.toNat && c.toNat <= 0x7E) || c == '\n' || c == '\t'

/-- The applicability test. -/
def applies (t : RunTimeText) : Bool :=
  t.reader != .personInBrowser && t.origin == .ownWording && !t.inTestCode

/-- The requirement. Whether the wording is English is left to review, because no definition
here decides it. The characters are decided. -/
def complies (t : RunTimeText) : Bool :=
  !applies t || t.notedDeviation || t.chars.all admitted

/-- A terminal message in English passes. -/
example : complies
    { chars := "apply: ran 4 instructions".toList, reader := .terminal,
      origin := .ownWording, inTestCode := false, notedDeviation := false } = true := by
  decide

/-- A log line in Japanese fails. -/
example : complies
    { chars := "命令を走らせる".toList, reader := .log,
      origin := .ownWording, inTestCode := false, notedDeviation := false } = false := by
  decide

/-- A Japanese file name passed through is outside the policy. -/
example : complies
    { chars := "メモ.md".toList, reader := .terminal,
      origin := .passedThrough, inTestCode := false, notedDeviation := false } = true := by
  decide

/-- A page shown to a person is outside the policy. -/
example : complies
    { chars := "検索".toList, reader := .personInBrowser,
      origin := .ownWording, inTestCode := false, notedDeviation := false } = true := by
  decide

end Policy.P0141TextAProgramEmitsIsEnglishAscii
