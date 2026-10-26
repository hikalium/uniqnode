/-
  should/0122-import-from-defining-module の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  Rust のコードの中の 1 つの `crate::X` 参照を主題にし、定義するモジュールのパスで参照し、
  main.rs のインポートの一覧を通してだけ解決する別名を使わないことを Prop にする。
-/
namespace ImportFromDefiningModule

/-- 参照がどう解決されるか。 -/
inductive Resolution where
  /-- 項目を定義するモジュールのパス (`use crate::task::task_field;`)。 -/
  | definingModule
  /-- 別のモジュール (main.rs など) のインポートの一覧を通してだけ解決する `crate::X`。 -/
  | importListAlias

/-- Rust のコードの中の 1 つの項目への参照。 -/
structure Reference where
  /-- 新しいコードか、組み直されているコードの中の参照か。 -/
  inNewOrRestructuredCode : Bool
  /-- 解決のしかた。 -/
  resolution : Resolution

/-- 適用のテスト。このリポジトリの Rust のうち、新しいコードと組み直されているコード。 -/
def Applies (r : Reference) : Prop :=
  r.inNewOrRestructuredCode = true

/-- 要求。項目は定義するモジュールのパスで参照する。インポートの掃除で止まった `crate::X` も同じ欠陥として直す。 -/
def Complies (r : Reference) : Prop :=
  Applies r -> r.resolution = Resolution.definingModule

/-- main.rs の残ったインポートを通して解決していた `crate::task_field` は、要求を満たさない。 -/
theorem alias_through_main_fails :
    Not (Complies { inNewOrRestructuredCode := true,
                    resolution := Resolution.importListAlias }) := by
  intro h
  cases h rfl

/-- 要求を満たす例。定義するモジュールを名指す `use crate::task::task_field;`。 -/
theorem defining_module_path_complies :
    Complies { inNewOrRestructuredCode := true, resolution := Resolution.definingModule } := by
  intro _
  rfl

/-- 触れていない既存のコードは対象外であり、何も求められない。 -/
theorem untouched_code_complies (s : Resolution) :
    Complies { inNewOrRestructuredCode := false, resolution := s } := by
  intro a
  cases a

end ImportFromDefiningModule
