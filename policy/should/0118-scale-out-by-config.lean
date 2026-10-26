/-
  should/0118-scale-out-by-config の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  似た実例を複数持てる資源の類への 1 つの実例の追加を主題にし、追加が設定の項目であること、
  できるなら実行時が特権のデプロイなしに設定の変更を拾うこと、特定の実例の名を持つ成果物を
  出荷前に直すことを Prop にする。
-/
namespace ScaleOutByConfig

/-- 実例を足す手段。 -/
inductive Means where
  /-- 設定ファイルの項目。 -/
  | configEntry
  /-- 新しいコード。 -/
  | newCode
  /-- 新しいサービスの unit。 -/
  | newServiceUnit
  /-- 実例ごとの成果物の複製。 -/
  | copiedArtifact

/-- 1 つの資源の類への、1 つの実例の追加。 -/
structure InstanceAddition where
  /-- 似た実例を複数持てる類 (サーバー、バックエンド、機器、複製) か。 -/
  classHasMultipleInstances : Bool
  /-- 足す手段。 -/
  addedBy : Means
  /-- 実行時が特権のデプロイなしに設定の変更を拾うことが、この類でできるか。 -/
  reloadWithoutRedeployFeasible : Bool
  /-- 実行時が特権のデプロイなしに設定の変更を拾うか。 -/
  reloadsWithoutPrivilegedRedeploy : Bool

/-- 適用のテスト。似た実例を複数持てる資源の類。 -/
def Applies (a : InstanceAddition) : Prop :=
  a.classHasMultipleInstances = true

/-- N 番目の実例は設定の項目で足す。 -/
def AddedByConfig (a : InstanceAddition) : Prop :=
  a.addedBy = Means.configEntry

/-- できるなら、実行時は特権のデプロイなしに設定の変更を拾う。 -/
def ReloadsWhereFeasible (a : InstanceAddition) : Prop :=
  a.reloadWithoutRedeployFeasible = true -> a.reloadsWithoutPrivilegedRedeploy = true

/-- 要求。 -/
def Complies (a : InstanceAddition) : Prop :=
  Applies a -> AddedByConfig a /\ ReloadsWhereFeasible a

/-- 最初の実例の設計。型を決める。 -/
structure FirstInstance where
  /-- 特定の実例の名を持つ成果物が、出荷の時点で残っているか。 -/
  shipsInstanceNamedArtifact : Bool

/-- 最初から列挙で設計し、特定の実例の名を持つ成果物は出荷前に直す。 -/
def FirstInstanceComplies (f : FirstInstance) : Prop :=
  f.shipsInstanceNamedArtifact = false

/-- 2 台目のバックエンドの機械に専用の unit ファイルを作ることは、要求を満たさない。 -/
theorem dedicated_unit_files_fail (f r : Bool) :
    Not (Complies { classHasMultipleInstances := true, addedBy := Means.newServiceUnit,
                    reloadWithoutRedeployFeasible := f, reloadsWithoutPrivilegedRedeploy := r }) := by
  intro h
  have a : Means.newServiceUnit = Means.configEntry := And.left (h rfl)
  cases a

/-- 拾えるのに特権のデプロイを要する設定の変更は、要求を満たさない。 -/
theorem privileged_redeploy_when_feasible_fails :
    Not (Complies { classHasMultipleInstances := true, addedBy := Means.configEntry,
                    reloadWithoutRedeployFeasible := true,
                    reloadsWithoutPrivilegedRedeploy := false }) := by
  intro h
  have r := And.right (h rfl)
  cases r rfl

/-- 要求を満たす例。既存の設定ファイルへの 2 行の項目で足し、実行時が再読込で拾う機械。 -/
theorem config_entry_complies :
    Complies { classHasMultipleInstances := true, addedBy := Means.configEntry,
               reloadWithoutRedeployFeasible := true, reloadsWithoutPrivilegedRedeploy := true } := by
  intro _
  exact And.intro rfl (fun _ => rfl)

/-- 1 つしか持てない類は対象外であり、何も求められない。 -/
theorem singleton_class_complies (m : Means) (f r : Bool) :
    Complies { classHasMultipleInstances := false, addedBy := m,
               reloadWithoutRedeployFeasible := f, reloadsWithoutPrivilegedRedeploy := r } := by
  intro a
  cases a

/-- 特定の実例の名を持つ成果物を残して出荷することは、要求を満たさない。 -/
theorem instance_named_artifact_fails :
    Not (FirstInstanceComplies { shipsInstanceNamedArtifact := true }) := by
  intro h
  cases h

end ScaleOutByConfig
