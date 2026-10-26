/-
  should/0137-prove-a-new-test-by-restoring-the-defect の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  修正に添えて足したテストが、欠陥を戻した木に対して赤になり修正を戻すと再び PASS すること、その結果が
  修正と共に運ばれること、機構ごとに実験を繰り返すこと、失敗に届くためだけの fixture に理由を残すことを
  Prop にする。
-/
namespace ProveANewTestByRestoringTheDefect

/-- テストを足した、または強めたと報告する 1 つの変更。 -/
structure TestAddition where
  /-- 「テストを足した」「テストを強めた」と報告しているか。 -/
  reportsTestAdded : Bool
  /-- 戻せる欠陥があるか。新機能の最初のテストには無い。 -/
  hasDefectToRestore : Bool
  /-- 欠陥を戻した木で新しいテストを走らせ、赤になったか。 -/
  failedAgainstRestoredDefect : Bool
  /-- 修正を戻した後、再び PASS することを検証したか。 -/
  passesAfterFixRestored : Bool
  /-- 「X を戻し、テスト Y が Z で落ちた」が修理の報告 (コミット、レビュー、タスクの記録) にあるか。 -/
  resultRecordedWithRepair : Bool
  /-- 欠陥が無いとき、代わりに何を変えて走らせたかをテストの脇に記したか。 -/
  variationRecordedAtTest : Bool
  /-- 所見が名指した機構の数。 -/
  mechanismsNamed : Nat
  /-- 欠陥を戻す実験で赤を見た機構の数。 -/
  mechanismsProven : Nat
  /-- その失敗に届くためだけの形を持つ fixture があるか。 -/
  hasFailureOnlyFixture : Bool
  /-- その fixture の脇に理由を残したか。 -/
  fixtureReasonRecorded : Bool

/-- 適用のテスト。テストを足した、または強めたと報告する変更。 -/
def Applies (c : TestAddition) : Prop :=
  c.reportsTestAdded = true

/-- 欠陥があるなら、戻した木に対してテストは赤になり、修正を戻すと再び PASS する。
    赤にならないなら修理は未完である。 -/
def BitesRestoredDefect (c : TestAddition) : Prop :=
  c.hasDefectToRestore = true ->
    c.failedAgainstRestoredDefect = true /\ c.passesAfterFixRestored = true

/-- 欠陥があるなら、実験の結果は修理の報告と共に運ばれる。 -/
def ResultTravels (c : TestAddition) : Prop :=
  c.hasDefectToRestore = true -> c.resultRecordedWithRepair = true

/-- 欠陥が無いなら、代わりに変えたものをテストの脇に記す。 -/
def VariationRecorded (c : TestAddition) : Prop :=
  c.hasDefectToRestore = false -> c.variationRecordedAtTest = true

/-- 名指された機構の数だけ実験を繰り返す。 -/
def OnePerMechanism (c : TestAddition) : Prop :=
  c.mechanismsProven = c.mechanismsNamed

/-- 失敗に届くためだけの fixture には、理由を残す。 -/
def FixtureReasonRecorded (c : TestAddition) : Prop :=
  c.hasFailureOnlyFixture = true -> c.fixtureReasonRecorded = true

/-- 要求。 -/
def Complies (c : TestAddition) : Prop :=
  Applies c ->
    BitesRestoredDefect c /\ ResultTravels c /\ VariationRecorded c /\ OnePerMechanism c /\
      FixtureReasonRecorded c

/-- 欠陥を戻しても PASS のままのテストは、足したと報告できない。 -/
theorem green_against_restored_defect_fails (recorded : Bool) (n : Nat) :
    Not (Complies { reportsTestAdded := true, hasDefectToRestore := true,
                    failedAgainstRestoredDefect := false, passesAfterFixRestored := true,
                    resultRecordedWithRepair := recorded, variationRecordedAtTest := true,
                    mechanismsNamed := n, mechanismsProven := n, hasFailureOnlyFixture := false,
                    fixtureReasonRecorded := false }) := by
  intro h
  have hr := h rfl
  cases And.left (hr.1 rfl)

/-- 5 つの経路を名指した監査の所見に 3 回の実験しか無いと、要求を満たさない。 -/
theorem unproven_mechanisms_fail :
    Not (Complies { reportsTestAdded := true, hasDefectToRestore := true,
                    failedAgainstRestoredDefect := true, passesAfterFixRestored := true,
                    resultRecordedWithRepair := true, variationRecordedAtTest := true,
                    mechanismsNamed := 5, mechanismsProven := 3, hasFailureOnlyFixture := false,
                    fixtureReasonRecorded := false }) := by
  intro h
  have hr := h rfl
  have e : 3 = 5 := hr.2.2.2.1
  exact absurd e (by decide)

/-- 要求を満たす例。古いアクセサを戻して 1 つのテストが落ち、修正を戻して PASS を検証し、その旨を
    コミットに書き、`loop_rust_fixture` の脇に理由を残した。 -/
theorem reverted_and_recorded_complies :
    Complies { reportsTestAdded := true, hasDefectToRestore := true,
               failedAgainstRestoredDefect := true, passesAfterFixRestored := true,
               resultRecordedWithRepair := true, variationRecordedAtTest := true,
               mechanismsNamed := 1, mechanismsProven := 1, hasFailureOnlyFixture := true,
               fixtureReasonRecorded := true } := by
  intro _
  exact And.intro (fun _ => And.intro rfl rfl)
    (And.intro (fun _ => rfl) (And.intro (fun _ => rfl) (And.intro rfl (fun _ => rfl))))

end ProveANewTestByRestoringTheDefect
