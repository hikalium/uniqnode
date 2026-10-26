/-
  should/0135-one-implementation-per-judgement の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  複数の場所で同じ答えを要る判定を構造にし、実装が 1 か所にあること、意図して残した 2 つ目は
  一致のテストと理由を持つこと、設計文書が 1 つの置き場を名指すことを要求として表す。
-/
namespace OneImplementationPerJudgement

/-- 機械が複数の場所で同じ答えを返すと期待される判定、変換、述語。 -/
structure Judgement where
  /-- 2 つ以上の場所が同じ答えを要るか。 -/
  neededAtSeveralSites : Bool
  /-- 共有するものがマーカーの文字列そのものか (must/0023 の領域)。 -/
  isMarkerWording : Bool
  /-- 問いが、既に 1 つの項目をどの経路でインポートするかだけか (should/0122 の領域)。 -/
  isOnlyImportPath : Bool
  /-- 答えを出す実装が 2 つ目として存在するか。 -/
  hasSecondImplementation : Bool
  /-- 1 つの置き場が、答える問いにちなんで名付けられているか (toolset::tool_is_usable など)。 -/
  homeNamedForQuestion : Bool
  /-- 1 つの置き場が、その問いを持つコードと共にあるか。 -/
  homeLivesWithOwningCode : Bool
  /-- 呼び出し側がその名前で置き場に届くか。 -/
  callersReachItByName : Bool
  /-- 2 つ目を意図して残しているか (本番の経路と単純な参照実装を比べるテストなど)。 -/
  secondKeptOnPurpose : Bool
  /-- 両方を走らせて一致を主張するテストが結んでいるか。 -/
  agreementTestTiesBoth : Bool
  /-- 外れた側に理由が記録されているか。 -/
  reasonRecordedAtDeviatingSite : Bool
  /-- 設計文書がこの判定の置き場を記しているか。 -/
  designDocPlacesJudgement : Bool
  /-- その設計文書が 1 つの置き場 (toolset::tool_is_usable など) を名指しているか。 -/
  designDocNamesSingleHome : Bool

/-- 適用のテスト。複数の場所が要る答えで、マーカーの文字列でもインポートの経路でもないもの。 -/
def Applies (j : Judgement) : Prop :=
  j.neededAtSeveralSites = true /\ j.isMarkerWording = false /\ j.isOnlyImportPath = false

/-- 決めるコードは 1 か所にある。意図して残した 2 つ目は、一致のテストと記録した理由で結ぶ。 -/
def SingleHome (j : Judgement) : Prop :=
  j.hasSecondImplementation = false \/
    (j.secondKeptOnPurpose = true /\ j.agreementTestTiesBoth = true /\
      j.reasonRecordedAtDeviatingSite = true)

/-- 1 つの置き場は問いにちなんで名付けられ、問いを持つコードと共にあり、呼び出し側はその名前で届く。 -/
def HomeWellPlaced (j : Judgement) : Prop :=
  j.homeNamedForQuestion = true /\ j.homeLivesWithOwningCode = true /\
    j.callersReachItByName = true

/-- 判定の置き場を記す設計文書は、1 つの置き場を名指す。 -/
def DesignDocNamesHome (j : Judgement) : Prop :=
  j.designDocPlacesJudgement = true -> j.designDocNamesSingleHome = true

/-- 要求。条項が当たる判定は、実装が 1 か所にあり、置き場が名前と場所を持ち、設計文書が 1 つの置き場を名指す。 -/
def Complies (j : Judgement) : Prop :=
  Applies j -> SingleHome j /\ HomeWellPlaced j /\ DesignDocNamesHome j

/-- 条項が止めたい形 (2026-08-03 のプロンプト側の再導出)。今日は一致している 2 つ目の実装が、
    テストも理由も無く残っている。 -/
def agreeingCopy : Judgement :=
  { neededAtSeveralSites := true, isMarkerWording := false, isOnlyImportPath := false,
    hasSecondImplementation := true, homeNamedForQuestion := false,
    homeLivesWithOwningCode := false, callersReachItByName := false,
    secondKeptOnPurpose := false,
    agreementTestTiesBoth := false, reasonRecordedAtDeviatingSite := false,
    designDocPlacesJudgement := false, designDocNamesSingleHome := false }

/-- 今日は一致している再導出も、テストと理由が無ければ要求を満たさない。 -/
theorem agreeing_copy_fails : Not (Complies agreeingCopy) := by
  intro h
  have applies : Applies agreeingCopy := And.intro rfl (And.intro rfl rfl)
  have single : true = false \/ (false = true /\ false = true /\ false = true) := (h applies).1
  cases single with
  | inl e => cases e
  | inr r => cases r.1

/-- 要求を満たす例。実行側が既に問う toolset::tool_is_usable を、プロンプト側も呼ぶ。 -/
theorem single_home_complies :
    Complies { neededAtSeveralSites := true, isMarkerWording := false, isOnlyImportPath := false,
               hasSecondImplementation := false, homeNamedForQuestion := true,
               homeLivesWithOwningCode := true, callersReachItByName := true,
               secondKeptOnPurpose := false,
               agreementTestTiesBoth := false, reasonRecordedAtDeviatingSite := false,
               designDocPlacesJudgement := true, designDocNamesSingleHome := true } := by
  intro _
  exact And.intro (Or.inl rfl) (And.intro (And.intro rfl (And.intro rfl rfl)) (fun _ => rfl))

/-- 意図して残した参照実装は、一致のテストと記録した理由があれば要求を満たす。 -/
theorem tied_reference_complies :
    Complies { neededAtSeveralSites := true, isMarkerWording := false, isOnlyImportPath := false,
               hasSecondImplementation := true, homeNamedForQuestion := true,
               homeLivesWithOwningCode := true, callersReachItByName := true,
               secondKeptOnPurpose := true,
               agreementTestTiesBoth := true, reasonRecordedAtDeviatingSite := true,
               designDocPlacesJudgement := false, designDocNamesSingleHome := false } := by
  intro _
  exact And.intro (Or.inr (And.intro rfl (And.intro rfl rfl)))
    (And.intro (And.intro rfl (And.intro rfl rfl)) (by intro e; cases e))

end OneImplementationPerJudgement
