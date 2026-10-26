/-
  should/0101-rust-first の Lean 版。uniqnode の条項には UUID の anchor が無い。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  Rust のコードへの 1 つの変更を主題にし、crate を足すか依存の木を広げるなら、std で足りない
  理由を commit message かコメントに記録する、という要求を Prop にする。
-/
namespace RustFirst

/-- 第一者の Rust のコード (agent runtime、supervisor、CLI、host の道具) か agent が納める Rust への 1 つの変更。 -/
structure Change where
  /-- 第一者の Rust か agent が納める Rust への変更か。 -/
  firstPartyRust : Bool
  /-- crate を足すか、既存の crate の依存の木を広げるか。 -/
  addsDependency : Bool
  /-- その crate が std では無理なく覆えない何を与えるかを、commit message かコードのコメントに記録したか。 -/
  justificationRecorded : Bool

/-- 適用のテスト。第一者の Rust か agent が納める Rust への変更。どの言語を使うかは must/0008 と must/0009 が決める。 -/
def Applies (c : Change) : Prop := c.firstPartyRust = true

/-- 要求。依存を足すなら、記録された理由がある。 -/
def Complies (c : Change) : Prop :=
  Applies c -> c.addsDependency = true -> c.justificationRecorded = true

/-- 理由を記録せずに crate を足した変更は、要求を満たさない。 -/
theorem unjustified_crate_fails :
    Not (Complies { firstPartyRust := true, addsDependency := true,
                    justificationRecorded := false }) := by
  intro h
  have e : false = true := h rfl rfl
  cases e

/-- 要求を満たす例。`std::process::Command` で外部の道具を呼び、何も足さない変更。 -/
theorem std_only_complies :
    Complies { firstPartyRust := true, addsDependency := false,
               justificationRecorded := false } := by
  intro _ e
  cases e

/-- 要求を満たす例。std で覆えないものを `Cargo.toml` の変更の commit message に書いて crate を足した変更。 -/
theorem justified_crate_complies :
    Complies { firstPartyRust := true, addsDependency := true,
               justificationRecorded := true } := by
  intro _ _
  exact rfl

end RustFirst
