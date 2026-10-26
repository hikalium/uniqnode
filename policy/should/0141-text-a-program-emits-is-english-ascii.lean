/-
  should/0141-text-a-program-emits-is-english-ascii の Lean の版。条項の本文は同じ名前の .md にあり、
  食い違ったら .md が優先する。lamalium の should/text-a-program-emits-is-english-ascii
  (uuid 9b8d9c9b-b173-4ac1-b81b-4ec78e94679b) と同じ規則である。実行時の文言の読み手と出どころを
  型で表し、適用のテストと要求を Prop にする。
-/
namespace TextAProgramEmitsIsEnglishAscii

/-- プログラムが動いているあいだに文言を読む相手。 -/
inductive Reader where
  /-- 端末で読む標準出力か標準エラー。 -/
  | terminal
  /-- journal やログのファイル。 -/
  | log
  /-- プログラムやエージェントが読む応答の本文。 -/
  | programOrAgent
  /-- モデルへ渡すプロンプト、ツールの説明、ツールの結果。 -/
  | model
  /-- ブラウザーが人に見せるページと、そのページが出すメッセージ。 -/
  | personInBrowser
  deriving DecidableEq

/-- 文言の出どころ。 -/
inductive Origin where
  /-- プログラム自身の文言。 -/
  | ownWording
  /-- 素通しの文。ファイル名、ファイルの中身、保存した文書、モデルの答えなど。 -/
  | passedThrough
  /-- 他者の言葉の代わりに置く文。利用者が言うと見込む言い回しなど。 -/
  | standsInForAnotherParty
  deriving DecidableEq

/-- 実行時に出す 1 つの文言と、それが置かれた場所。 -/
structure RunTimeText where
  chars : List Char
  reader : Reader
  origin : Origin
  /-- テストのコードの中のメッセージか。 -/
  inTestCode : Bool
  /-- 逸脱 (別の言語の文を判定するプログラムがその言語の語を挙げるなど) を、理由とともに記したか。 -/
  notedDeviation : Bool

/-- 要求が認める文字。印字できる ASCII と、改行とタブ。 -/
def admitted (c : Char) : Bool :=
  (0x20 <= c.toNat && c.toNat <= 0x7E) || c == '\n' || c == '\t'

/-- 適用のテスト。ブラウザーのページの外で、プログラム自身の文言で、テストのコードの外にあるもの。 -/
abbrev Applies (t : RunTimeText) : Prop :=
  Not (t.reader = Reader.personInBrowser) /\ t.origin = Origin.ownWording /\ t.inTestCode = false

/-- 要求。条項が当たる文言は、記した逸脱でなければ、認める文字だけで書く。英語かどうかはレビューが
  判定し、ここでは文字だけを決める。 -/
abbrev Complies (t : RunTimeText) : Prop :=
  Applies t -> t.notedDeviation = true \/ t.chars.all admitted = true

/-- 端末へ出す英語のメッセージは要求を満たす。 -/
theorem english_terminal_message_complies :
    Complies { chars := "apply: ran 4 instructions".toList, reader := .terminal,
               origin := .ownWording, inTestCode := false, notedDeviation := false } := by
  decide

/-- ログへ出す日本語の行は要求を満たさない。 -/
theorem japanese_log_line_does_not_comply :
    Not (Complies { chars := "命令を走らせる".toList, reader := .log,
                    origin := .ownWording, inTestCode := false, notedDeviation := false }) := by
  decide

/-- 素通しの日本語のファイル名は条項の外にあるので、要求を満たす。 -/
theorem passed_through_file_name_complies :
    Complies { chars := "メモ.md".toList, reader := .terminal,
               origin := .passedThrough, inTestCode := false, notedDeviation := false } := by
  decide

/-- 人に見せるページの文言は条項の外にあるので、要求を満たす。 -/
theorem browser_page_complies :
    Complies { chars := "検索".toList, reader := .personInBrowser,
               origin := .ownWording, inTestCode := false, notedDeviation := false } := by
  decide

end TextAProgramEmitsIsEnglishAscii
