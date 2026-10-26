/-
  must/0012-no-opaque-jargon-churn の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  利用者とのやり取りと木の中の文書とコメントの中の 1 つの語を主題にして、適用のテストと要求を Prop にする。
-/
namespace NoOpaqueJargonChurn

/-- 利用者とのやり取りか、木の中の文書とコメントの中の 1 つの語。 -/
structure Term where
  /-- 利用者とのやり取りか、木の中の文書やコメントの中にあるか。 -/
  inCoveredText : Bool
  /-- 内部を知らない読み手が、その語から振る舞いを取り出せるか。厳密に使った標準の語は取り出せる。 -/
  readerWithoutInternalsUnderstands : Bool
  /-- 曖昧な何でも入る語か。 -/
  isVagueCatchAll : Bool
  /-- 記録された禁止の例である "churn" そのものか。 -/
  isTheWordChurn : Bool

/-- 適用のテスト。覆う文の中の語。 -/
def Applies (t : Term) : Prop :=
  t.inCoveredText = true

/-- 読み手が解読せずに振る舞いを取り出せる。 -/
def ReaderRecoversBehaviour (t : Term) : Prop :=
  t.readerWithoutInternalsUnderstands = true

/-- 曖昧な語でも "churn" でもない。 -/
def NotACatchAll (t : Term) : Prop :=
  t.isVagueCatchAll = false /\ t.isTheWordChurn = false

/-- 要求。振る舞いを平易で具体的な語で名指す。 -/
def Complies (t : Term) : Prop :=
  Applies t -> ReaderRecoversBehaviour t /\ NotACatchAll t

/-- "churn" は、要求を満たさない。 -/
theorem churn_fails :
    Not (Complies { inCoveredText := true, readerWithoutInternalsUnderstands := false,
                    isVagueCatchAll := true, isTheWordChurn := true }) := by
  intro h
  have e : true = false := (h rfl).2.2
  cases e

/-- 内部を知る者にしか解読できない社内の略語は、曖昧でなくても要求を満たさない。 -/
theorem in_house_shorthand_fails :
    Not (Complies { inCoveredText := true, readerWithoutInternalsUnderstands := false,
                    isVagueCatchAll := false, isTheWordChurn := false }) := by
  intro h
  have e : false = true := (h rfl).1
  cases e

/-- 要求を満たす例。「空回り」(busy but making no progress) と振る舞いを名指した語。 -/
theorem named_behaviour_complies :
    Complies { inCoveredText := true, readerWithoutInternalsUnderstands := true,
               isVagueCatchAll := false, isTheWordChurn := false } := by
  intro _
  exact And.intro rfl (And.intro rfl rfl)

/-- 要求を満たす例。標準の語 "thrashing" を厳密な意味で使った語。 -/
theorem thrashing_in_exact_sense_complies :
    Complies { inCoveredText := true, readerWithoutInternalsUnderstands := true,
               isVagueCatchAll := false, isTheWordChurn := false } := by
  intro _
  exact And.intro rfl (And.intro rfl rfl)

end NoOpaqueJargonChurn
