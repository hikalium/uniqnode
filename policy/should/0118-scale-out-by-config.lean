/-
  should/0118-scale-out-by-config の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  似た実例を複数持てる資源の類への 1 つの実例の追加を主題にし、追加が設定の項目であること、
  特定の実例の名を持つ成果物を出荷前に直すことを Prop にする。
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

/-- 適用のテスト。似た実例を複数持てる資源の類。 -/
def Applies (a : InstanceAddition) : Prop :=
  a.classHasMultipleInstances = true

/-- 要求。N 番目の実例は設定の項目で足す。 -/
def Complies (a : InstanceAddition) : Prop :=
  Applies a -> a.addedBy = Means.configEntry

/-- 最初の実例の設計。型を決める。 -/
structure FirstInstance where
  /-- 特定の実例の名を持つ成果物が、出荷の時点で残っているか。 -/
  shipsInstanceNamedArtifact : Bool

/-- 最初から列挙で設計し、特定の実例の名を持つ成果物は出荷前に直す。 -/
def FirstInstanceComplies (f : FirstInstance) : Prop :=
  f.shipsInstanceNamedArtifact = false

/-- 2 台目のバックエンドの機械に専用の unit ファイルを作ることは、要求を満たさない。 -/
theorem dedicated_unit_files_fail :
    Not (Complies { classHasMultipleInstances := true, addedBy := Means.newServiceUnit }) := by
  intro h
  cases h rfl

/-- 実例ごとの成果物を複製して足すことも、要求を満たさない。 -/
theorem copied_artifact_fails :
    Not (Complies { classHasMultipleInstances := true, addedBy := Means.copiedArtifact }) := by
  intro h
  cases h rfl

/-- 要求を満たす例。既存の設定ファイルへの 2 行の項目で足した機械。 -/
theorem config_entry_complies :
    Complies { classHasMultipleInstances := true, addedBy := Means.configEntry } := by
  intro _
  rfl

/-- 1 つしか持てない類は対象外であり、何も求められない。 -/
theorem singleton_class_complies (m : Means) :
    Complies { classHasMultipleInstances := false, addedBy := m } := by
  intro a
  cases a

/-- 特定の実例の名を持つ成果物を残して出荷することは、要求を満たさない。 -/
theorem instance_named_artifact_fails :
    Not (FirstInstanceComplies { shipsInstanceNamedArtifact := true }) := by
  intro h
  cases h

end ScaleOutByConfig
