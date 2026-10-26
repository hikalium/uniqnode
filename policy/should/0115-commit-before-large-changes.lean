/-
  should/0115-commit-before-large-changes の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  大きな変更の着手と、1 つのコミットの粒度の 2 つを主題にして、適用のテストと要求を Prop にする。
-/
namespace CommitBeforeLargeChanges

/-- git のリポジトリで始めようとしている 1 つの仕事。 -/
structure Start where
  /-- 大きな仕事 (リファクタ、再編成、複数ファイルの機能) か。 -/
  isSubstantial : Bool
  /-- 着手の前に `git status` で木の状態を見たか。 -/
  treeChecked : Bool
  /-- これから始める仕事とは別のコミットに属する未コミットの変更があるか。 -/
  unrelatedUncommittedPresent : Bool
  /-- その変更を先にコミットしたか。 -/
  unrelatedCommittedFirst : Bool

/-- 適用のテスト。大きな仕事の着手。 -/
def Applies (s : Start) : Prop :=
  s.isSubstantial = true

/-- 着手の前に木を見る。 -/
def TreeChecked (s : Start) : Prop :=
  s.treeChecked = true

/-- 別の仕事の変更が残っていれば先にコミットし、2 つの仕事を同じ汚れた木で混ぜない。 -/
def PreviousCampaignCommitted (s : Start) : Prop :=
  s.unrelatedUncommittedPresent = true -> s.unrelatedCommittedFirst = true

/-- 要求。 -/
def Complies (s : Start) : Prop :=
  Applies s -> TreeChecked s /\ PreviousCampaignCommitted s

/-- 1 つのコミット。 -/
structure Commit where
  /-- 1 つのまとまった意図の単位か。 -/
  isOneCoherentUnit : Bool
  /-- 単独で成り立つ複数の単位を 1 つにまとめているか。 -/
  bundlesStandaloneUnits : Bool

/-- 粒度への要求。意図を下限として、細かいほうを選ぶ。 -/
def CommitComplies (c : Commit) : Prop :=
  c.isOneCoherentUnit = true /\ c.bundlesStandaloneUnits = false

/-- 別の仕事の変更が残る木で大きな仕事を始めるのは、要求を満たさない。 -/
theorem interleaving_campaigns_fails :
    Not (Complies { isSubstantial := true, treeChecked := true,
                    unrelatedUncommittedPresent := true, unrelatedCommittedFirst := false }) := by
  intro h
  have e : false = true := (h rfl).2 rfl
  cases e

/-- 木を見ずに始めるのも、要求を満たさない。 -/
theorem unchecked_start_fails :
    Not (Complies { isSubstantial := true, treeChecked := false,
                    unrelatedUncommittedPresent := false, unrelatedCommittedFirst := false }) := by
  intro h
  have e : false = true := (h rfl).1
  cases e

/-- 単独で成り立つ複数の単位をまとめたコミットは、要求を満たさない。 -/
theorem coarse_commit_fails :
    Not (CommitComplies { isOneCoherentUnit := true, bundlesStandaloneUnits := true }) := by
  intro h
  have e : true = false := h.2
  cases e

/-- 要求を満たす例。木を見て、前の仕事をコミットしてから始める大きな変更。 -/
theorem checked_and_committed_start_complies :
    Complies { isSubstantial := true, treeChecked := true,
               unrelatedUncommittedPresent := true, unrelatedCommittedFirst := true } := by
  intro _
  exact And.intro rfl (fun _ => rfl)

/-- 要求を満たす例。1 つの意図からなるコミット。 -/
theorem coherent_commit_complies :
    CommitComplies { isOneCoherentUnit := true, bundlesStandaloneUnits := false } :=
  And.intro rfl rfl

end CommitBeforeLargeChanges
