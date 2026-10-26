/-
  must/0022-no-silent-error-swallowing の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。Result を持つソースの 1 か所と
  そのエラー側の扱いを型で表し、黙って捨てない要求を Prop にする。
-/
namespace NoSilentErrorSwallowing

/-- コードが Result のエラー側をどう扱うか。 -/
inductive Disposition where
  /-- `?` で呼び出し側へ伝える。 -/
  | propagated
  /-- その場で扱う。分岐する、報告する、意図して代替の値に落とす。 -/
  | handledAtSite
  /-- `best_effort("what", <result>)` で、消さずに記録する。 -/
  | loggedBestEffort
  /-- `let x = foo.ok();` で Option に変換する。None の場合をその後で扱うかを持つ。 -/
  | boundToOption (noneHandled : Bool)
  /-- 文 `EXPR.ok();` で None にして床に落とす。 -/
  | okStatement

/-- Result を持つソースの 1 か所。 -/
structure ResultSite where
  /-- このリポジトリの Rust のソース (container/src/、container/tests/、container/build.rs) の
  中か。 -/
  inRustSource : Bool
  /-- エラー側の扱い。 -/
  disposition : Disposition

/-- 失敗の痕跡がログ、計器、戻り値のどれかに残る扱いか。 -/
def Disposition.leavesTrace : Disposition -> Prop
  | .propagated => True
  | .handledAtSite => True
  | .loggedBestEffort => True
  | .boundToOption noneHandled => noneHandled = true
  | .okStatement => False

/-- 適用のテスト。Rust のソースで Result を持つ場所。 -/
def Applies (s : ResultSite) : Prop :=
  s.inRustSource = true

/-- 要求。条項が当たる場所は、エラーを黙って捨てない。 -/
def Complies (s : ResultSite) : Prop :=
  Applies s -> s.disposition.leavesTrace

/-- 文 `EXPR.ok();` は要求を満たさない。 -/
theorem ok_statement_fails :
    Not (Complies { inRustSource := true, disposition := Disposition.okStatement }) := by
  intro h
  exact h rfl

/-- None の場合を扱わずに運ぶ変換は、要求を満たさない。 -/
theorem unhandled_bound_conversion_fails :
    Not (Complies { inRustSource := true,
                    disposition := Disposition.boundToOption false }) := by
  intro h
  cases h rfl

/-- 要求を満たす例。`?` で伝える。 -/
theorem propagation_complies :
    Complies { inRustSource := true, disposition := Disposition.propagated } := by
  intro _
  exact trivial

/-- 要求を満たす例。`let x = foo.ok();` の後で None の場合に分岐する。 -/
theorem handled_bound_conversion_complies :
    Complies { inRustSource := true, disposition := Disposition.boundToOption true } := by
  intro _
  exact rfl

/-- 要求を満たす例。`best_effort` で記録する。 -/
theorem best_effort_log_complies :
    Complies { inRustSource := true, disposition := Disposition.loggedBestEffort } := by
  intro _
  exact trivial

/-- Rust のソースの外に書かれた例 (文書の中の `.ok();`) は対象外である。 -/
theorem fixture_string_outside :
    Complies { inRustSource := false, disposition := Disposition.okStatement } := by
  intro e
  cases e

end NoSilentErrorSwallowing
