/-
  should/0104-no-unjustified-waits の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  コードの中の待ちを種類で分け、条件を待つ形か、呼び出し元に理由を書いた 3 種の時間待ちだけを
  認めることを要求として表す。
-/
namespace NoUnjustifiedWaits

/-- 待ちの種類。 -/
inductive WaitKind where
  /-- 定まった条件 (ファイルが現れる、状態が変わる、ログの事象が届く、ポートが答える) を待ち、
      成り立った瞬間に進む。 -/
  | conditionPoll
  /-- 制御ループの刻み。条件を再検査する間隔 (supervisor のバスの掃除の間の sleep(2) など)。 -/
  | pollInterval
  /-- 条件待ちが相手の故障で永久に止まらないための安全の上限。 -/
  | safetyTimeout
  /-- ハードウェアや手順が外から定める落ち着き時間。 -/
  | settleTime
  /-- 条件も理由も無い固定の sleep (「落ち着くまで」「念のため」)。 -/
  | fixedDelay

/-- コードの中の 1 つの待ち。 -/
structure Wait where
  /-- lamalium のコード (supervisor、エージェント、ホストの道具、container/src/*.rs) の中か。 -/
  inFirstPartyCode : Bool
  /-- 種類。 -/
  kind : WaitKind
  /-- 呼び出し元に短い理由が書いてあるか。 -/
  justifiedAtCallSite : Bool
  /-- 成功したら早く抜けるか (安全の上限のとき)。 -/
  returnsEarlyOnSuccess : Bool
  /-- 期限切れで静かに進まず、大きく失敗するか (安全の上限のとき)。 -/
  failsLoudlyOnExpiry : Bool
  /-- 出典を引いているか (落ち着き時間のとき)。 -/
  sourceCited : Bool

/-- 適用のテスト。lamalium のコードの中の待ち。 -/
def Applies (w : Wait) : Prop :=
  w.inFirstPartyCode = true

/-- 認められる待ち。条件待ちはそのまま、時間待ちは種類ごとの条件つき、固定の遅延は認めない。 -/
def Acceptable (w : Wait) : Prop :=
  match w.kind with
  | .conditionPoll => True
  | .pollInterval => w.justifiedAtCallSite = true
  | .safetyTimeout =>
      w.justifiedAtCallSite = true /\ w.returnsEarlyOnSuccess = true /\
        w.failsLoudlyOnExpiry = true
  | .settleTime => w.justifiedAtCallSite = true /\ w.sourceCited = true
  | .fixedDelay => False

/-- 要求。条項が当たる待ちは、認められる形のどれかである。 -/
def Complies (w : Wait) : Prop :=
  Applies w -> Acceptable w

/-- 条項が止めたい形。`sleep(N)`「念のため」は、理由めいた言葉があっても要求を満たさない。 -/
theorem fixed_delay_fails (justified : Bool) :
    Not (Complies { inFirstPartyCode := true, kind := WaitKind.fixedDelay,
                    justifiedAtCallSite := justified, returnsEarlyOnSuccess := false,
                    failsLoudlyOnExpiry := false, sourceCited := false }) := by
  intro h
  exact h rfl

/-- 期限切れで静かに進む上限は、安全網ではないので要求を満たさない。 -/
theorem silent_timeout_fails :
    Not (Complies { inFirstPartyCode := true, kind := WaitKind.safetyTimeout,
                    justifiedAtCallSite := true, returnsEarlyOnSuccess := true,
                    failsLoudlyOnExpiry := false, sourceCited := false }) := by
  intro h
  have e : false = true := (h rfl).2.2
  cases e

/-- 要求を満たす例。ポートが答えるまで細かく繰り返し、答えた瞬間に進む。 -/
theorem condition_poll_complies :
    Complies { inFirstPartyCode := true, kind := WaitKind.conditionPoll,
               justifiedAtCallSite := false, returnsEarlyOnSuccess := true,
               failsLoudlyOnExpiry := true, sourceCited := false } := by
  intro _
  exact trivial

/-- 要求を満たす例。理由を書いた supervisor の sleep(2) の刻み。 -/
theorem poll_interval_complies :
    Complies { inFirstPartyCode := true, kind := WaitKind.pollInterval,
               justifiedAtCallSite := true, returnsEarlyOnSuccess := false,
               failsLoudlyOnExpiry := false, sourceCited := false } := by
  intro _
  exact rfl

end NoUnjustifiedWaits
