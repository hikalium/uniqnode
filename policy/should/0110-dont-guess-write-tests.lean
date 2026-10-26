/-
  should/0110-dont-guess-write-tests の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  原因や修正の 1 つの主張を主題にし、推論だけで断定せず、論理ならテストで、テストが合わなければ
  直接の観測で証明することを Prop にする。 PASS のビルドや終了コード 0 は代理であり、証明ではない。
-/
namespace DontGuessWriteTests

/-- 主張を支えている根拠。 -/
inductive Evidence where
  /-- 推論だけで、何も走らせていない。 -/
  | reasoningOnly
  /-- 修正無しで落ち、修正で通るテストを書いて走らせた。「落ちる」側は欠陥を戻して検証する (should/0137)。 -/
  | test
  /-- 一時的な探りやログや断定で、実際の値を読んだ。 -/
  | directObservation
  /-- PASS のビルドや終了コード 0 のような代理。 -/
  | proxy

/-- 原因の断定、または修正が効くという宣言。 -/
structure Claim where
  /-- 主張する前に、なぜそう振る舞うか、変更が本当に効くかが分かっていなかったか。 -/
  wasUncertain : Bool
  /-- 振る舞いを小さな純粋関数に取り出して単体テストにできるか。 -/
  unitTestFits : Bool
  /-- 主張を支えている根拠。 -/
  evidence : Evidence

/-- 適用のテスト。分かっていないまま出そうとする、原因や修正の主張。 -/
def Applies (c : Claim) : Prop :=
  c.wasUncertain = true

/-- 論理なら、テストで証明する。 -/
def LogicIsTested (c : Claim) : Prop :=
  c.unitTestFits = true -> c.evidence = Evidence.test

/-- 単体テストが合わないなら、直接の観測で証明する。 -/
def OtherwiseObserved (c : Claim) : Prop :=
  c.unitTestFits = false -> c.evidence = Evidence.directObservation

/-- 要求。推論だけでも、代理だけでも、主張を出さない。 -/
def Complies (c : Claim) : Prop :=
  Applies c -> LogicIsTested c /\ OtherwiseObserved c

/-- 「正しく見える」推論だけの原因の断定は、要求を満たさない。 -/
theorem reasoning_alone_fails (fits : Bool) :
    Not (Complies { wasUncertain := true, unitTestFits := fits,
                    evidence := Evidence.reasoningOnly }) := by
  intro h
  have hr := h rfl
  cases fits with
  | true => cases hr.1 rfl
  | false => cases hr.2 rfl

/-- PASS のビルドを証明に使った修正の宣言は、要求を満たさない。 -/
theorem proxy_fails (fits : Bool) :
    Not (Complies { wasUncertain := true, unitTestFits := fits, evidence := Evidence.proxy }) := by
  intro h
  have hr := h rfl
  cases fits with
  | true => cases hr.1 rfl
  | false => cases hr.2 rfl

/-- 要求を満たす例。URL のクエリの解析を純粋関数に取り出し、5 行のテストで上流の削除を示した。 -/
theorem parse_tested_complies :
    Complies { wasUncertain := true, unitTestFits := true, evidence := Evidence.test } := by
  intro _
  exact And.intro (fun _ => rfl) (fun e => by cases e)

/-- 要求を満たす例。テストが合わない振る舞いを、一時的なログで実際の値を読んで決めた。 -/
theorem observed_value_complies :
    Complies { wasUncertain := true, unitTestFits := false,
               evidence := Evidence.directObservation } := by
  intro _
  exact And.intro (fun e => by cases e) (fun _ => rfl)

end DontGuessWriteTests
