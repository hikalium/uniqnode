/-
  should/0137-prove-a-new-test-by-restoring-the-defect の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  修正に添えて足したテストが、欠陥を戻した木に対して赤になること、その結果が修正と共に運ばれること、
  機構ごとに実験を繰り返すことを Prop にする。
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
  /-- 「X を戻し、テスト Y が Z で落ちた」が修理の報告 (コミット、レビュー、タスクの記録) にあるか。 -/
  resultRecordedWithRepair : Bool
  /-- 欠陥が無いとき、代わりに何を変えて走らせたかをテストの脇に記したか。 -/
  variationRecordedAtTest : Bool
  /-- 所見が名指した機構の数。 -/
  mechanismsNamed : Nat
  /-- 欠陥を戻す実験で赤を見た機構の数。 -/
  mechanismsProven : Nat

/-- 適用のテスト。テストを足した、または強めたと報告する変更。 -/
def Applies (c : TestAddition) : Prop :=
  c.reportsTestAdded = true

/-- 欠陥があるなら、戻した木に対してテストは赤になる。 PASS のままなら修理は未完である。 -/
def BitesRestoredDefect (c : TestAddition) : Prop :=
  c.hasDefectToRestore = true -> c.failedAgainstRestoredDefect = true

/-- 欠陥があるなら、実験の結果は修理の報告と共に運ばれる。 -/
def ResultTravels (c : TestAddition) : Prop :=
  c.hasDefectToRestore = true -> c.resultRecordedWithRepair = true

/-- 欠陥が無いなら、代わりに変えたものをテストの脇に記す。 -/
def VariationRecorded (c : TestAddition) : Prop :=
  c.hasDefectToRestore = false -> c.variationRecordedAtTest = true

/-- 名指された機構の数だけ実験を繰り返す。 -/
def OnePerMechanism (c : TestAddition) : Prop :=
  c.mechanismsProven = c.mechanismsNamed

/-- 要求。 -/
def Complies (c : TestAddition) : Prop :=
  Applies c ->
    BitesRestoredDefect c /\ ResultTravels c /\ VariationRecorded c /\ OnePerMechanism c

/-- 欠陥を戻しても PASS のままのテストは、足したと報告できない。 -/
theorem green_against_restored_defect_fails (recorded : Bool) (n : Nat) :
    Not (Complies { reportsTestAdded := true, hasDefectToRestore := true,
                    failedAgainstRestoredDefect := false, resultRecordedWithRepair := recorded,
                    variationRecordedAtTest := true, mechanismsNamed := n,
                    mechanismsProven := n }) := by
  intro h
  have hr := h rfl
  cases hr.1 rfl

/-- 5 つの経路を名指した監査の所見に 3 回の実験しか無いと、要求を満たさない。 -/
theorem unproven_mechanisms_fail :
    Not (Complies { reportsTestAdded := true, hasDefectToRestore := true,
                    failedAgainstRestoredDefect := true, resultRecordedWithRepair := true,
                    variationRecordedAtTest := true, mechanismsNamed := 5,
                    mechanismsProven := 3 }) := by
  intro h
  have hr := h rfl
  have e : 3 = 5 := hr.2.2.2
  exact absurd e (by decide)

/-- 要求を満たす例。古いアクセサを戻して 1 つのテストが落ち、その旨をコミットに書いた。 -/
theorem reverted_and_recorded_complies :
    Complies { reportsTestAdded := true, hasDefectToRestore := true,
               failedAgainstRestoredDefect := true, resultRecordedWithRepair := true,
               variationRecordedAtTest := true, mechanismsNamed := 1,
               mechanismsProven := 1 } := by
  intro _
  exact And.intro (fun _ => rfl) (And.intro (fun _ => rfl) (And.intro (fun _ => rfl) rfl))

end ProveANewTestByRestoringTheDefect
