/-
  must/0021-no-markdown-emphasis の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  プロジェクトが作る文字列の中の 1 つのマークを主題にし、散文の強調のためのマークを禁じる要求を Prop にする。
-/
namespace NoMarkdownEmphasis

/-- 文字列の一部に付いている Markdown のマーク。 -/
inductive Decoration where
  /-- マークが無い。 -/
  | none
  /-- 太字のマーク (`**...**`、`__...__`)。 -/
  | boldMarkers
  /-- 斜体のマーク (`*...*`、`_..._`)。 -/
  | italicMarkers

/-- そのマークが文字列の中で果たしている役割。 -/
inductive Purpose where
  /-- 散文に重みを付ける。これが強調である。 -/
  | emphasis
  /-- 見出し、箇条書き、表、コードフェンス、バッククォートの範囲。 -/
  | structural
  /-- glob、コード、正規表現、下線を含む識別子などの技術的な意味。 -/
  | technicalLiteral
  /-- バイト単位で一致させる必要のある既存の文の引用。 -/
  | verbatimQuotation

/-- プロジェクトが作る文字列の中の 1 つの範囲。 -/
structure Span where
  /-- プロジェクトが作る文字列 (プロンプト、条項、文書、コメント、コミットメッセージ、返信) の中か。 -/
  inProjectString : Bool
  /-- 付いているマーク。 -/
  decoration : Decoration
  /-- マークの役割。 -/
  purpose : Purpose

/-- 適用のテスト。プロジェクトが作る文字列の中で、マークが散文の強調に使われているか。 -/
def Applies (s : Span) : Prop :=
  s.inProjectString = true /\ s.purpose = Purpose.emphasis

/-- 要求。強調のためにマークを付けない。重みが要る文は、位置か独立した文で重みを得る。 -/
def Complies (s : Span) : Prop :=
  Applies s -> s.decoration = Decoration.none

/-- 散文の強調のための太字は、要求を満たさない。 -/
theorem bold_for_emphasis_fails :
    Not (Complies { inProjectString := true, decoration := Decoration.boldMarkers,
                    purpose := Purpose.emphasis }) := by
  intro h
  cases h (And.intro rfl rfl)

/-- 散文の強調のための斜体も、要求を満たさない。 -/
theorem italic_for_emphasis_fails :
    Not (Complies { inProjectString := true, decoration := Decoration.italicMarkers,
                    purpose := Purpose.emphasis }) := by
  intro h
  cases h (And.intro rfl rfl)

/-- 要求を満たす例。重みを位置で得て、マークを持たない文。 -/
theorem plain_sentence_complies :
    Complies { inProjectString := true, decoration := Decoration.none,
               purpose := Purpose.emphasis } := by
  intro _
  rfl

/-- バッククォートの中の glob の `*` は強調ではなく、条項の対象外なので、何も求められない。 -/
theorem glob_in_backticks_complies (d : Decoration) :
    Complies { inProjectString := true, decoration := d, purpose := Purpose.technicalLiteral } := by
  intro h
  cases And.right h

/-- バイト単位で一致させる引用の中のマークも、対象外である。 -/
theorem verbatim_quotation_complies (d : Decoration) :
    Complies { inProjectString := true, decoration := d, purpose := Purpose.verbatimQuotation } := by
  intro h
  cases And.right h

end NoMarkdownEmphasis
