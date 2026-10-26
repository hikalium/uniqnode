/-
  must/0025-no-heredoc-for-text-with-commands の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  貼り付け用のコマンドのブロックを持つファイルへの編集を構造にし、その中身がシェルの解釈する
  経路 (heredoc、echo、本文を埋め込んだスクリプト) を通らないこと、編集するプログラム自身も
  先にファイルの道具で書くこと、そのファイルを編集しうるサブエージェントへの指示がこの規則を
  述べることを要求として表す。
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
  /-- 編集するプログラムを、走らせる前にファイルの道具でファイルへ書いたか (プログラムの経路のとき)。 -/
  programWrittenWithFileTools : Bool

/-- 適用のテスト。貼り付け用のコマンドのブロックを持つファイルへの編集。 -/
def Applies (e : Edit) : Prop :=
  e.fileCarriesCommandBlocks = true

/-- 編集するプログラム自身も、先にファイルの道具でファイルへ書いてから走らせる。 -/
def ProgramFromFile (e : Edit) : Prop :=
  e.route = EditRoute.programReadingDisk -> e.programWrittenWithFileTools = true

/-- 要求。中身はシェルが解釈しない経路でだけ届き、プログラムは先にファイルへ書く。 -/
def Complies (e : Edit) : Prop :=
  Applies e -> e.route.safe /\ ProgramFromFile e

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

/-- 本文を heredoc で渡す編集は、本文の EOF で閉じて残りがシェルで走るので要求を満たさない。 -/
theorem heredoc_edit_fails :
    Not (Complies { fileCarriesCommandBlocks := true, route := EditRoute.shellHeredoc,
                    programWrittenWithFileTools := false }) := by
  intro h
  exact (h rfl).1

/-- 本文を埋め込んだスクリプトも同じく要求を満たさない。 -/
theorem embedding_script_fails :
    Not (Complies { fileCarriesCommandBlocks := true, route := EditRoute.scriptEmbeddingText,
                    programWrittenWithFileTools := true }) := by
  intro h
  exact (h rfl).1

/-- 条項が止めたい形 (2026-10-01 の SYSTEMD.md)。ディスクを読む編集のスクリプトでも、それ自身を
    heredoc で渡せば要求を満たさない。 -/
theorem heredoc_delivered_script_fails :
    Not (Complies { fileCarriesCommandBlocks := true, route := EditRoute.programReadingDisk,
                    programWrittenWithFileTools := false }) := by
  intro h
  have e : false = true := (h rfl).2 rfl
  cases e

/-- 要求を満たす例。docs/mop/ の手順を Edit で直す。 -/
theorem file_tool_edit_complies :
    Complies { fileCarriesCommandBlocks := true, route := EditRoute.fileTool,
               programWrittenWithFileTools := false } := by
  intro _
  exact And.intro trivial (by intro e; cases e)

/-- 要求を満たす例。先に Write でファイルへ書いたスクリプトを走らせて編集する。 -/
theorem script_from_file_complies :
    Complies { fileCarriesCommandBlocks := true, route := EditRoute.programReadingDisk,
               programWrittenWithFileTools := true } := by
  intro _
  exact And.intro trivial (fun _ => rfl)

/-- コマンドのブロックを持たないファイルへの heredoc は、この条項の対象の外にある。 -/
theorem plain_file_outside :
    Not (Applies { fileCarriesCommandBlocks := false, route := EditRoute.shellHeredoc,
                   programWrittenWithFileTools := false }) := by
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
