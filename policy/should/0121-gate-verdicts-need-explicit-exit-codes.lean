/-
  should/0121-gate-verdicts-need-explicit-exit-codes の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  重大な動作を門で止めるシェルの列と、沈黙を情報として読む見張りを構造にし、段階ごとの終了状態が
  判定役に届くこと、一度赤を見て検証したことを要求として表す。
-/
namespace GateVerdictsNeedExplicitExitCodes

/-- 検査の終了状態が判定役に届く経路。 -/
inductive StatusPath where
  /-- 段階ごとに終了状態を明示して取り (status=$?)、それで判定する。 -/
  | explicitPerStep
  /-- pipefail を有効にしたパイプの列で判定する。 -/
  | pipefailChain
  /-- パイプの最後のコマンド (tail、grep、head) の終了状態で判定する。 -/
  | lastOfPipe
  /-- `timeout ...; echo $?` のように、別のコマンドの終了状態を報告する。 -/
  | echoedStatus

/-- 重大な動作 (コミット、デプロイ、検査の PASS、関所の通過) を門で止めるシェルの列。 -/
structure Verdict where
  /-- その結果が重大な動作を門で止めるか。 -/
  gatesConsequentialAction : Bool
  /-- 終了状態が判定役に届く経路。 -/
  path : StatusPath
  /-- 門を作った後に一度わざと失敗させ、止まることを検証したか。 -/
  seenRedOnce : Bool

/-- 検査器自身の終了状態が判定役に届く経路か。 -/
def StatusPath.reachesJudge : StatusPath -> Prop
  | .explicitPerStep => True
  | .pipefailChain => True
  | .lastOfPipe => False
  | .echoedStatus => False

/-- 適用のテスト。重大な動作を門で止めるシェルの列。 -/
def Applies (v : Verdict) : Prop :=
  v.gatesConsequentialAction = true

/-- 要求。終了状態が判定役に届く経路で判定し、一度赤を見て検証してある。 -/
def Complies (v : Verdict) : Prop :=
  Applies v -> v.path.reachesJudge /\ v.seenRedOnce = true

/-- 沈黙が情報として読まれる見張り (監視の grep、警報、完了の知らせ)。 -/
structure Watcher where
  /-- その沈黙が「起きていない」と読まれるか。 -/
  silenceReadAsInformation : Bool
  /-- 条件を一度起こして、鳴ることを検証したか。 -/
  firedOnceWhenTriggered : Bool
  /-- 検証のあいだに stderr を捨てていたか。 -/
  stderrDiscardedWhileProving : Bool

/-- 適用のテスト。沈黙が情報として読まれる見張り。 -/
def WatcherApplies (w : Watcher) : Prop :=
  w.silenceReadAsInformation = true

/-- 要求。沈黙を信じる前に一度鳴らし、その検証で stderr を捨てない。 -/
def WatcherComplies (w : Watcher) : Prop :=
  WatcherApplies w -> w.firedOnceWhenTriggered = true /\ w.stderrDiscardedWhileProving = false

/-- 条項が止めたい形 (2026-07-19)。`clippy | tail -2 && git commit` は tail の 0 で赤をコミットする。 -/
theorem pipe_tail_gate_fails (seen : Bool) :
    Not (Complies { gatesConsequentialAction := true, path := StatusPath.lastOfPipe,
                    seenRedOnce := seen }) := by
  intro h
  exact (h rfl).1

/-- 一度も赤を見ていない門は、経路が正しくても検証されていない。 -/
theorem unseen_red_gate_fails :
    Not (Complies { gatesConsequentialAction := true, path := StatusPath.explicitPerStep,
                    seenRedOnce := false }) := by
  intro h
  have e : false = true := (h rfl).2
  cases e

/-- 要求を満たす例。段階ごとに終了状態を取って判定し、一度わざと赤にして止まるのを見た門。 -/
theorem explicit_status_gate_complies :
    Complies { gatesConsequentialAction := true, path := StatusPath.explicitPerStep,
               seenRedOnce := true } := by
  intro _
  exact And.intro trivial rfl

/-- `ctl log --limit` のように存在しないフラグに結ばれ、stderr を捨てたまま一度も鳴らなかった
    見張りは、要求を満たさない。 -/
theorem silent_watcher_fails :
    Not (WatcherComplies { silenceReadAsInformation := true, firedOnceWhenTriggered := false,
                           stderrDiscardedWhileProving := true }) := by
  intro h
  have e : false = true := (h rfl).1
  cases e

/-- 要求を満たす例。条件を一度起こして鳴るのを見た見張り。 -/
theorem proven_watcher_complies :
    WatcherComplies { silenceReadAsInformation := true, firedOnceWhenTriggered := true,
                      stderrDiscardedWhileProving := false } := by
  intro _
  exact And.intro rfl rfl

end GateVerdictsNeedExplicitExitCodes
