/-
  should/0125-what-makes-a-good-test の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  1 つのテストの文面が持つべき性質 (振る舞いを調べる、失敗が自分で語る、決定的、小さい) と、
  頼る振る舞いにはテストを置くことを Prop にする。
-/
namespace WhatMakesAGoodTest

/-- 書かれた、またはレビューされる 1 つのテスト。 -/
structure Test where
  /-- 利用者や呼び手が観測できる振る舞いを調べているか。 -/
  probesObservableBehavior : Bool
  /-- 内部の変数名や関数名を断定しているか。 -/
  assertsInternalNames : Bool
  /-- 識別子 (セレクタ、id) を名指しているか。 -/
  namesIdentifier : Bool
  /-- 名指した識別子を失敗の詳細に明記しているか。 -/
  identifierStatedInFailureDetail : Bool
  /-- 失敗が、破られた期待を 1 文で述べるメッセージを持つか。 -/
  failureHasMessage : Bool
  /-- 失敗が、行った探りと実際に見つかったものを失敗の時点で集めた詳細を持つか。 -/
  failureHasDetail : Bool
  /-- 壁時計、実行順、以前の実行に依らないか。待ちは期限付きで条件を見るか。 -/
  deterministic : Bool
  /-- 1 つの検査が 1 つの振る舞いだけを見るか。 -/
  oneBehaviorPerCheck : Bool

/-- 適用のテスト。この条項は、書かれレビューされる全てのテストに当たる。 -/
def Applies (_t : Test) : Prop :=
  True

/-- 実装ではなく振る舞いを調べる。識別子を名指すなら、失敗の詳細に明記する。 -/
def TestsBehavior (t : Test) : Prop :=
  t.probesObservableBehavior = true /\ t.assertsInternalNames = false /\
    (t.namesIdentifier = true -> t.identifierStatedInFailureDetail = true)

/-- 失敗が修理の話を自分で語る。 -/
def FailureTellsStory (t : Test) : Prop :=
  t.failureHasMessage = true /\ t.failureHasDetail = true

/-- 決定的で、小さい。 -/
def DeterministicAndSmall (t : Test) : Prop :=
  t.deterministic = true /\ t.oneBehaviorPerCheck = true

/-- 要求。 -/
def Complies (t : Test) : Prop :=
  Applies t -> TestsBehavior t /\ FailureTellsStory t /\ DeterministicAndSmall t

/-- 頼られている 1 つの振る舞い。 -/
structure Behavior where
  /-- 誰かがその振る舞いに頼っているか。 -/
  reliedOn : Bool
  /-- その振る舞いにテストが置かれているか。 -/
  tested : Bool

/-- 適用のテスト。頼られている振る舞い。 -/
def BehaviorApplies (b : Behavior) : Prop :=
  b.reliedOn = true

/-- 頼る振る舞いにはテストを置く。 -/
def BehaviorComplies (b : Behavior) : Prop :=
  BehaviorApplies b -> b.tested = true

/-- 内部の変数名を断定し、失敗に詳細を付けないテスト (2026-07-20 の実例) は、要求を満たさない。 -/
theorem asserts_internal_name_fails :
    Not (Complies { probesObservableBehavior := false, assertsInternalNames := true,
                    namesIdentifier := false, identifierStatedInFailureDetail := false,
                    failureHasMessage := true, failureHasDetail := false,
                    deterministic := true, oneBehaviorPerCheck := true }) := by
  intro h
  have hr := h trivial
  cases And.left hr.1

/-- 判定が揺れるテストは、無いほうがましであり、要求を満たさない。 -/
theorem flaky_test_fails :
    Not (Complies { probesObservableBehavior := true, assertsInternalNames := false,
                    namesIdentifier := false, identifierStatedInFailureDetail := false,
                    failureHasMessage := true, failureHasDetail := true,
                    deterministic := false, oneBehaviorPerCheck := true }) := by
  intro h
  have hr := h trivial
  cases And.left hr.2.2

/-- 要求を満たす例。要素の存在を調べ、セレクタを失敗の詳細に書き、期限付きで待つ検査。 -/
theorem observable_check_complies :
    Complies { probesObservableBehavior := true, assertsInternalNames := false,
               namesIdentifier := true, identifierStatedInFailureDetail := true,
               failureHasMessage := true, failureHasDetail := true,
               deterministic := true, oneBehaviorPerCheck := true } := by
  intro _
  exact And.intro (And.intro rfl (And.intro rfl (fun _ => rfl)))
    (And.intro (And.intro rfl rfl) (And.intro rfl rfl))

/-- 頼られているのにテストの無い振る舞いは、要求を満たさない。 -/
theorem untested_reliance_fails :
    Not (BehaviorComplies { reliedOn := true, tested := false }) := by
  intro h
  cases h rfl

end WhatMakesAGoodTest
