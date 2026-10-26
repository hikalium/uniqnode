/-
  should/0117-mechanize-manual-repairs の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。環境への手作業の 1 つの修復を型で表し、
  適用のテストと要求 (再構築の経路に機械化されて初めて終わる) を Prop にする。
-/
namespace MechanizeManualRepairs

/-- 環境の欠陥への 1 つの修復。 -/
structure Repair where
  /-- その環境が自動化 (provisioning、bootstrap、イメージのビルド) で作り直されるものか。 -/
  environmentRebuiltByAutomation : Bool
  /-- 手で実行した修復か。 -/
  doneByHand : Bool
  /-- 環境を作り直す全ての経路に、冪等で実行可能な操作として組み込んだか。 -/
  wiredIntoEveryRebuildPath : Bool
  /-- 文書、記憶、警告のコメントとしてだけ残っているか。次の再構築は警告を読まない。 -/
  existsOnlyAsProse : Bool

/-- 適用のテスト。自動化で作り直される環境への、手作業の修復。 -/
def Applies (r : Repair) : Prop :=
  r.environmentRebuiltByAutomation = true /\ r.doneByHand = true

/-- 修復は、環境を作り直す全ての経路に冪等な操作として組み込まれている。 -/
def WiredIntoRebuild (r : Repair) : Prop :=
  r.wiredIntoEveryRebuildPath = true

/-- 散文だけの修復は対策ではない。 -/
def NotOnlyProse (r : Repair) : Prop :=
  r.existsOnlyAsProse = false

/-- 要求。 -/
def Complies (r : Repair) : Prop :=
  Applies r -> WiredIntoRebuild r /\ NotOnlyProse r

/-- 2026-07 の実例。手で消したロックファイルを docs に書いただけの修復は、要求を満たさない。
    再構築の手順そのものが損傷を作り直した。 -/
theorem repair_recorded_only_in_docs_fails :
    Not (Complies { environmentRebuiltByAutomation := true, doneByHand := true,
                    wiredIntoEveryRebuildPath := false, existsOnlyAsProse := true }) := by
  intro h
  cases (h (And.intro rfl rfl)).1

/-- 要求を満たす例。手での修復を、bootstrap とイメージのビルドの両方に冪等な段階として組み込んだ。 -/
theorem repair_wired_into_rebuild_complies :
    Complies { environmentRebuiltByAutomation := true, doneByHand := true,
               wiredIntoEveryRebuildPath := true, existsOnlyAsProse := false } := by
  intro _
  exact And.intro rfl rfl

/-- 自動化で作り直されない環境への修復には当たらない。 -/
theorem hand_built_environment_is_out_of_scope :
    Complies { environmentRebuiltByAutomation := false, doneByHand := true,
               wiredIntoEveryRebuildPath := false, existsOnlyAsProse := true } := by
  intro h
  cases h.1

end MechanizeManualRepairs
