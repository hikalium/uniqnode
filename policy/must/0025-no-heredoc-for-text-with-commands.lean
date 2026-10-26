/-
  must/0025-no-heredoc-for-text-with-commands の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  貼り付け用のコマンドのブロックを持つファイルへの編集を構造にし、その中身がシェルの解釈する
  経路 (heredoc、echo、本文を埋め込んだスクリプト) を通らないことと、そのファイルを編集しうる
  サブエージェントへの指示がこの規則を述べることを要求として表す。
-/
namespace NoHeredocForTextWithCommands

/-- 編集の中身がファイルへ届く経路。 -/
inductive EditRoute where
  /-- 編集者のファイルの道具 (Edit、Write、エディタ)。 -/
  | fileTool
  /-- ディスクからファイルを読んで書き戻すプログラム。 -/
  | programReadingDisk
  /-- シェルの heredoc (`<<EOF` など)。 -/
  | shellHeredoc
  /-- `echo` など、シェルが本文を解釈する経路。 -/
  | shellEcho
  /-- その本文を埋め込んだスクリプト。 -/
  | scriptEmbeddingText

/-- シェルが本文を解釈しない経路か。 -/
def EditRoute.safe : EditRoute -> Prop
  | .fileTool => True
  | .programReadingDisk => True
  | .shellHeredoc => False
  | .shellEcho => False
  | .scriptEmbeddingText => False

/-- 人、assistant、エージェントによる 1 つのファイルの編集。 -/
structure Edit where
  /-- 誰かが貼り付けるためのコマンドのブロック (docs/mop/ の `sudo ... bash -s` など) を持つファイルか。 -/
  fileCarriesCommandBlocks : Bool
  /-- 中身がファイルへ届く経路。 -/
  route : EditRoute

/-- 適用のテスト。貼り付け用のコマンドのブロックを持つファイルへの編集。 -/
def Applies (e : Edit) : Prop :=
  e.fileCarriesCommandBlocks = true

/-- 要求。中身はシェルが解釈しない経路でだけ届く。 -/
def Complies (e : Edit) : Prop :=
  Applies e -> e.route.safe

/-- そのようなファイルを編集しうるサブエージェントへの指示。 -/
structure SubagentInstruction where
  /-- 受け手がそのようなファイルを編集しうるか。 -/
  subagentMayEditSuchFile : Bool
  /-- 指示がこの規則を述べているか。 -/
  statesRule : Bool

/-- 適用のテスト。そのようなファイルを編集しうるサブエージェントへの指示。 -/
def InstructionApplies (i : SubagentInstruction) : Prop :=
  i.subagentMayEditSuchFile = true

/-- 要求。指示はこの規則を述べる。 -/
def InstructionComplies (i : SubagentInstruction) : Prop :=
  InstructionApplies i -> i.statesRule = true

/-- 条項が止めたい形 (2026-10-01 の SYSTEMD.md)。編集のスクリプトを heredoc で渡し、本文の EOF で
    閉じて残りが root で走る。 -/
theorem heredoc_edit_fails :
    Not (Complies { fileCarriesCommandBlocks := true, route := EditRoute.shellHeredoc }) := by
  intro h
  exact h rfl

/-- 本文を埋め込んだスクリプトも同じく要求を満たさない。 -/
theorem embedding_script_fails :
    Not (Complies { fileCarriesCommandBlocks := true, route := EditRoute.scriptEmbeddingText }) := by
  intro h
  exact h rfl

/-- 要求を満たす例。docs/mop/ の手順を Edit で直す。 -/
theorem file_tool_edit_complies :
    Complies { fileCarriesCommandBlocks := true, route := EditRoute.fileTool } := by
  intro _
  exact trivial

/-- コマンドのブロックを持たないファイルへの heredoc は、この条項の対象の外にある。 -/
theorem plain_file_outside :
    Not (Applies { fileCarriesCommandBlocks := false, route := EditRoute.shellHeredoc }) := by
  intro e
  cases e

/-- 規則を述べない指示は、要求を満たさない。 -/
theorem silent_instruction_fails :
    Not (InstructionComplies { subagentMayEditSuchFile := true, statesRule := false }) := by
  intro h
  have e : false = true := h rfl
  cases e

/-- 要求を満たす例。規則を 1 文で述べた指示。 -/
theorem stated_instruction_complies :
    InstructionComplies { subagentMayEditSuchFile := true, statesRule := true } := by
  intro _
  exact rfl

end NoHeredocForTextWithCommands
