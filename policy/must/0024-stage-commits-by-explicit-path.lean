/-
  must/0024-stage-commits-by-explicit-path の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  コミットに入れるファイルを選ぶ形を型にし、パスを名指して stage すること、コミット前に
  確認すること、要る変更だけを入れること、削除も名指すことを要求として表す。
-/
namespace StageCommitsByExplicitPath

/-- コミットに入れるファイルを選んだ形。 -/
inductive StagingForm where
  /-- `git add <path> ...` で各ファイルを名指した。 -/
  | namedPaths
  /-- `git add -A` か `git add --all`。 -/
  | addAll
  /-- `git add .`。 -/
  | addDot
  /-- リポジトリの根で `git add -u`。 -/
  | addUpdateAtRoot
  /-- `git commit -a`。 -/
  | commitAll

/-- 書き手が明示したパスで選んだ形か。 -/
def StagingForm.explicit : StagingForm -> Prop
  | .namedPaths => True
  | .addAll => False
  | .addDot => False
  | .addUpdateAtRoot => False
  | .commitAll => False

/-- このリポジトリで作る 1 つのコミット (人でも assistant でもエージェントでも、代わりに
    stage する道具でも)。 -/
structure Commit where
  /-- 選んだ形。 -/
  form : StagingForm
  /-- コミット前に `git status --short` と `git diff --cached --stat` を見たか。 -/
  stagedFilesConfirmed : Bool
  /-- 変更が要るファイルだけを入れたか。 -/
  onlyRequiredFiles : Bool
  /-- 削除を含むか。 -/
  hasDeletion : Bool
  /-- 削除も `git rm <path>` か `git add <deleted-path>` で名指したか。 -/
  deletionNamed : Bool

/-- 適用のテスト。このリポジトリで作るコミットの全て。 -/
def Applies (_c : Commit) : Prop :=
  True

/-- 削除は、その消したパスを名指して stage する。 -/
def DeletionNamed (c : Commit) : Prop :=
  c.hasDeletion = true -> c.deletionNamed = true

/-- 要求。パスを名指して選び、確認し、要るものだけを入れ、削除も名指す。 -/
def Complies (c : Commit) : Prop :=
  Applies c ->
    c.form.explicit /\ c.stagedFilesConfirmed = true /\ c.onlyRequiredFiles = true /\
      DeletionNamed c

/-- 条項が止めたい形 (2026-08-06 の 280a38b)。`git add -A` は、確認して要るものだけに見えても
    要求を満たさない。 -/
theorem add_all_fails (confirmed required : Bool) :
    Not (Complies { form := StagingForm.addAll, stagedFilesConfirmed := confirmed,
                    onlyRequiredFiles := required, hasDeletion := false,
                    deletionNamed := false }) := by
  intro h
  exact (h trivial).1

/-- `git commit -a` も同じく要求を満たさない。 -/
theorem commit_all_fails :
    Not (Complies { form := StagingForm.commitAll, stagedFilesConfirmed := true,
                    onlyRequiredFiles := true, hasDeletion := false,
                    deletionNamed := false }) := by
  intro h
  exact (h trivial).1

/-- 名指しで選んでも、確認を省くと要求を満たさない。 -/
theorem unconfirmed_fails :
    Not (Complies { form := StagingForm.namedPaths, stagedFilesConfirmed := false,
                    onlyRequiredFiles := true, hasDeletion := false,
                    deletionNamed := false }) := by
  intro h
  have e : false = true := (h trivial).2.1
  cases e

/-- 要求を満たす例。2 つのファイルを名指して add し、status と cached diff を見てからコミットする。 -/
theorem named_paths_complies :
    Complies { form := StagingForm.namedPaths, stagedFilesConfirmed := true,
               onlyRequiredFiles := true, hasDeletion := true, deletionNamed := true } := by
  intro _
  exact And.intro trivial (And.intro rfl (And.intro rfl (fun _ => rfl)))

end StageCommitsByExplicitPath
