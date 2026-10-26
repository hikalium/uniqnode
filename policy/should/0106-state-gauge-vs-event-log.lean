/-
  should/0106-state-gauge-vs-event-log の Lean 版。uniqnode の条項には UUID の anchor が無い。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  実行時の状態の 1 回の書き出しと 1 回の読み出しを主題にし、いまの状態は gauge に、
  イベントログには変化と一度きりの出来事だけを、という 2 種類のデータの厳密な分離を Prop にする。
-/
namespace StateGaugeVsEventLog

/-- 書き出す先の種類。 -/
inductive Store where
  /-- gauge。その場で上書きされ、読み手が引く snapshot (`status.json`、`/metrics`、`/api/tree`)。 -/
  | gauge
  /-- イベントログ。追記だけの時系列 (`agent.jsonl`、`supervisor.jsonl`、feed)。 -/
  | eventLog

/-- 書き出す内容の種類。 -/
inductive DataKind where
  /-- いまの状態。前の tick で既に真だったことの再掲を含む。 -/
  | currentState
  /-- 状態の遷移 (idle から busy へ、paused になった、device が落ちた、エラー)。 -/
  | transition
  /-- 一度きりの出来事 (step、steer、bind)。 -/
  | oneShotEvent

/-- 実行時の状態を記録する側の 1 回の書き出し。 -/
structure Emission where
  /-- agent、supervisor、device、task、bus の実行時の状態を記録するか。 -/
  recordsRuntimeState : Bool
  /-- 書く内容。 -/
  kind : DataKind
  /-- 書く先。 -/
  store : Store

/-- 適用のテスト。実行時の状態を記録する書き出し。 -/
def Applies (e : Emission) : Prop := e.recordsRuntimeState = true

/-- 要求。いまの状態は gauge にだけ書き、gauge にはいまの状態だけを書く。遷移と出来事はイベントログに行く。 -/
def Complies (e : Emission) : Prop :=
  Applies e ->
    (e.kind = DataKind.currentState -> e.store = Store.gauge) /\
    (e.store = Store.gauge -> e.kind = DataKind.currentState)

/-- いまの状態を読む側の 1 回の読み出し。 -/
structure Read where
  /-- いまの状態 (agent の状態、device の up と down、heartbeat、queue の深さ) を読むか。 -/
  readsCurrentState : Bool
  /-- 読む元。 -/
  source : Store

/-- 適用のテスト。いまの状態を読む読み出し。 -/
def ReadApplies (r : Read) : Prop := r.readsCurrentState = true

/-- いまの状態は gauge から読み、ログから組み立て直さない。 -/
def ReadComplies (r : Read) : Prop := ReadApplies r -> r.source = Store.gauge

/-- 毎 tick の idle や paused の再掲をイベントログに書くことは、要求を満たさない。 -/
theorem per_tick_snapshot_in_log_fails :
    Not (Complies { recordsRuntimeState := true, kind := DataKind.currentState,
                    store := Store.eventLog }) := by
  intro h
  have e : Store.eventLog = Store.gauge := (h rfl).1 rfl
  cases e

/-- 一度きりの出来事を gauge に上書きで書くことは、要求を満たさない。 -/
theorem event_in_gauge_fails :
    Not (Complies { recordsRuntimeState := true, kind := DataKind.oneShotEvent,
                    store := Store.gauge }) := by
  intro h
  have e : DataKind.oneShotEvent = DataKind.currentState := (h rfl).2 rfl
  cases e

/-- UI がログを走査していまの状態を組み立て直すことは、要求を満たさない。 -/
theorem reconstructing_state_from_log_fails :
    Not (ReadComplies { readsCurrentState := true, source := Store.eventLog }) := by
  intro h
  have e : Store.eventLog = Store.gauge := h rfl
  cases e

/-- 要求を満たす例。遷移だけをイベントログに書く。 -/
theorem transition_in_log_complies :
    Complies { recordsRuntimeState := true, kind := DataKind.transition,
               store := Store.eventLog } := by
  intro _
  exact And.intro (fun e => by cases e) (fun e => by cases e)

/-- 要求を満たす例。heartbeat を毎回 gauge に上書きする。 -/
theorem heartbeat_gauge_complies :
    Complies { recordsRuntimeState := true, kind := DataKind.currentState,
               store := Store.gauge } := by
  intro _
  exact And.intro (fun _ => rfl) (fun _ => rfl)

/-- 要求を満たす例。UI が gauge の snapshot からいまの状態を読む。 -/
theorem ui_reads_gauge_complies :
    ReadComplies { readsCurrentState := true, source := Store.gauge } := by
  intro _
  exact rfl

end StateGaugeVsEventLog
