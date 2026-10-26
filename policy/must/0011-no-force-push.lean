/-
  must/0011-no-force-push の Lean 版。uniqnode の条項には UUID の anchor が無い。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  remote への 1 回の push を主題にし、push 済みの履歴を書き換えないこと、直しは前向きの commit で
  行うこと、push 前の commit は自由に直してよいことを Prop にする。
-/
namespace NoForcePush

/-- push 済みの commit の誤りを直す手。 -/
inductive Fix where
  /-- 新しい前向きの commit か `git revert`。 -/
  | forwardCommit
  /-- amend、rebase、reset で書き換えてから push する。 -/
  | rewrite

/-- remote への 1 回の push。 -/
structure Push where
  /-- 動かそうとする履歴が、既に remote へ push 済みか (`main` を含む)。 -/
  historyPublished : Bool
  /-- `--force` か `--force-with-lease` を使うか。 -/
  forced : Bool
  /-- push 済みの commit を amend、rebase、reset で書き換えた結果を押すか。 -/
  rewritesPublishedCommit : Bool

/-- 適用のテスト。push 済みの履歴に触れる push。まだ push していない commit には当たらない。 -/
def Applies (p : Push) : Prop := p.historyPublished = true

/-- 要求。push 済みの履歴は不変で、force も書き換えもしない。 -/
def Complies (p : Push) : Prop :=
  Applies p -> p.forced = false /\ p.rewritesPublishedCommit = false

/-- push 済みの commit の誤りを直す 1 回の手。 -/
structure Correction where
  /-- 直す commit が push 済みか。 -/
  commitPublished : Bool
  /-- 選んだ手。 -/
  fix : Fix

/-- 適用のテスト。push 済みの commit の直し。 -/
def CorrectionApplies (c : Correction) : Prop := c.commitPublished = true

/-- push 済みの commit は前向きの commit で直す。 -/
def CorrectionComplies (c : Correction) : Prop :=
  CorrectionApplies c -> c.fix = Fix.forwardCommit

/-- `main` への force push は要求を満たさない。 -/
theorem force_push_fails (rewrites : Bool) :
    Not (Complies { historyPublished := true, forced := true,
                    rewritesPublishedCommit := rewrites }) := by
  intro h
  have e : true = false := (h rfl).1
  cases e

/-- push 済みの commit を rebase してから押すことは、force の有無にかかわらず要求を満たさない。 -/
theorem rewritten_history_fails (forced : Bool) :
    Not (Complies { historyPublished := true, forced := forced,
                    rewritesPublishedCommit := true }) := by
  intro h
  have e : true = false := (h rfl).2
  cases e

/-- push 済みの commit を書き換えで直すことは、要求を満たさない。 -/
theorem rewrite_as_correction_fails :
    Not (CorrectionComplies { commitPublished := true, fix := Fix.rewrite }) := by
  intro h
  have e : Fix.rewrite = Fix.forwardCommit := h rfl
  cases e

/-- 要求を満たす例。新しい commit を fast-forward で push する。 -/
theorem forward_push_complies :
    Complies { historyPublished := true, forced := false, rewritesPublishedCommit := false } := by
  intro _
  exact And.intro rfl rfl

/-- 要求を満たす例。まだ push していない commit を amend してから初めて push する。条項は当たらない。 -/
theorem local_amend_complies (forced : Bool) :
    Complies { historyPublished := false, forced := forced, rewritesPublishedCommit := true } := by
  intro e
  cases e

/-- 要求を満たす例。push 済みの誤りを `git revert` で直す。 -/
theorem revert_complies :
    CorrectionComplies { commitPublished := true, fix := Fix.forwardCommit } := by
  intro _
  exact rfl

end NoForcePush
