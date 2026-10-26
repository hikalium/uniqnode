/-
  should/0107-japanese-prose-style の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。日本語の文章の中の 1 つの語を
  構造体で表し、その語の形が許されるかを Prop にする。
-/
namespace JapaneseProseStyle

/-- 日本語の文章に書かれた 1 つの語の形。 -/
inductive Form where
  /-- 明らかに広く使われているカタカナ語や固有の構成要素名をそのまま使った形 (タスク、デプロイ、
  supervisor)。 -/
  | establishedTerm
  /-- 普通の日本語 (「調停する」「起動する」「束縛する」「割り当てる」「購読する」)。 -/
  | plainJapanese
  /-- 広く使われているとは言えない英字語・カタカナ語の動詞 (リコンサイルする、アクティベートする、
  バインドする、サブスクライブする)。 -/
  | forcedLoanword
  /-- 定着した技術語の過剰な翻訳 (押済み履歴、配備、基線)。 -/
  | overTranslation
  /-- ソースコードの識別子・設定キー・状態値を原語のまま置いた形 (spawn_subtask)。 -/
  | identifierVerbatim
  /-- 識別子を訳した形。grep できない別の語になる。 -/
  | identifierTranslated

/-- 1 つの語の出現。 -/
structure Word where
  /-- ユーザーとの日本語のやり取りか、日本語で書く文書・コメントの中か。英語の設計文書・コード・
  識別子そのものは対象外。 -/
  inJapaneseProse : Bool
  /-- その語の形。 -/
  form : Form

/-- 条項が許す形か。定着した語はそのまま、普通の日本語はそのまま、識別子は原語のまま。 -/
def Form.allowed : Form -> Prop
  | .establishedTerm => True
  | .plainJapanese => True
  | .forcedLoanword => False
  | .overTranslation => False
  | .identifierVerbatim => True
  | .identifierTranslated => False

/-- 適用のテスト。日本語の文章の中の語。 -/
def Applies (w : Word) : Prop :=
  w.inJapaneseProse = true

/-- 要求。条項が当たる語は、許される形で書かれている。 -/
def Complies (w : Word) : Prop :=
  Applies w -> w.form.allowed

/-- 無理に散りばめたカタカナ語の動詞 (リコンサイルする、バインドする) は要求を満たさない。 -/
theorem forced_loanword_fails :
    Not (Complies { inJapaneseProse := true, form := Form.forcedLoanword }) := by
  intro h
  exact h rfl

/-- 定着した技術語の過剰な翻訳 (配備、基線) も要求を満たさない。 -/
theorem over_translation_fails :
    Not (Complies { inJapaneseProse := true, form := Form.overTranslation }) := by
  intro h
  exact h rfl

/-- 識別子を訳した形は要求を満たさない。 -/
theorem translated_identifier_fails :
    Not (Complies { inJapaneseProse := true, form := Form.identifierTranslated }) := by
  intro h
  exact h rfl

/-- 要求を満たす例。動詞を日本語で書く (「束縛する」「割り当てる」「調停する」)。 -/
theorem plain_japanese_complies :
    Complies { inJapaneseProse := true, form := Form.plainJapanese } := by
  intro _
  exact trivial

/-- 要求を満たす例。定着したカタカナ名詞 (デプロイ) をそのまま使う。 -/
theorem established_term_complies :
    Complies { inJapaneseProse := true, form := Form.establishedTerm } := by
  intro _
  exact trivial

/-- 要求を満たす例。識別子 `state="queued"` を原語のまま置く。 -/
theorem verbatim_identifier_complies :
    Complies { inJapaneseProse := true, form := Form.identifierVerbatim } := by
  intro _
  exact trivial

/-- 英語のコードや設計文書の中の語は対象外で、どの形でも要求に触れない。 -/
theorem outside_japanese_prose_complies (f : Form) :
    Complies { inJapaneseProse := false, form := f } := by
  intro e
  cases e

end JapaneseProseStyle
