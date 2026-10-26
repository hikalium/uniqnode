/-
  should/0129-alerts-fire-on-transitions の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。アラートを出す機会を型で表し、
  遷移のときだけ出すという要求と、逸脱の 2 つの形を Prop にする。段階の大きさや間隔の値 (log_volume_alarm の
  1 GB、6 時間、10 パーセント、24 時間) は出す側の定数にあり、ここでは扱わない。
-/
namespace AlertsFireOnTransitions

/-- 監視している条件を評価した 1 回の機会。 -/
inductive Occasion where
  /-- 悪い状態へ初めて入った。 -/
  | firstCrossing
  /-- 設計時に決めた段階を 1 つ越えて悪化した。 -/
  | tierCrossed
  /-- ヒステリシスの下まで戻って ok になった。 -/
  | recovery
  /-- 割合などの条件で、クールダウンを過ぎてから再び越えた。 -/
  | rateAfterCooldown
  /-- 悪い状態が続く中で、思い出させる間隔 (例えば 24 時間) が過ぎた。 -/
  | reminderDue
  /-- 同じ状態のままの再評価。 -/
  | sameState
  /-- 割合などの条件で、クールダウンの中で再び越えた。 -/
  | rateInsideCooldown
  /-- 思い出させる間隔が過ぎる前の再評価。 -/
  | reminderEarly

/-- 1 つのアラートの経路 (feed の event、goal_alarm、log_volume_alarm、escalation) が、ある機会にしたこと。 -/
structure Alert where
  /-- その機会。 -/
  occasion : Occasion
  /-- 出したか。 -/
  fired : Bool
  /-- 消費者 (誰が読んで何をするか) を設計時に名指したか (should/0111)。 -/
  consumerNamed : Bool
  /-- 遷移の表をテストで留めてあるか (同じ状態は黙る、段階の越えは出す、クールダウンの中は黙る、
      思い出させるのは間隔の後だけ)。 -/
  transitionTableTested : Bool
  /-- 起きるたびに独立して対処する離散の出来事か (失敗したバックアップ、セキュリティの拒否)。 -/
  discreteEvent : Bool
  /-- 離散の出来事であることを、出す場所に書いてあるか。 -/
  notedAtEmittingSite : Bool
  /-- 本当に重大な条件として、より強く繰り返すことの正当化を記録してあるか。 -/
  criticalRepeatJustified : Bool

/-- その機会が、条件の遷移 (報せるべき新しい情報) か。 -/
def Occasion.isTransition : Occasion -> Bool
  | .firstCrossing => true
  | .tierCrossed => true
  | .recovery => true
  | .rateAfterCooldown => true
  | .reminderDue => true
  | .sameState => false
  | .rateInsideCooldown => false
  | .reminderEarly => false

/-- 適用のテスト。全てのアラートの経路。 -/
def Applies (_a : Alert) : Prop := True

/-- 遷移のときに出し、遷移でないときは黙る。離散の出来事と、正当化を記録した重大な条件は逸脱として除く。 -/
def TransitionRule (a : Alert) : Prop :=
  a.discreteEvent = false -> a.criticalRepeatJustified = false -> a.fired = a.occasion.isTransition

/-- 離散の出来事として毎回出すなら、出す場所にそう書く。 -/
def DiscreteEventNoted (a : Alert) : Prop :=
  a.discreteEvent = true -> a.notedAtEmittingSite = true

/-- 消費者を設計時に名指す。 -/
def ConsumerNamed (a : Alert) : Prop :=
  a.consumerNamed = true

/-- 遷移の表はテストで留める。 -/
def TransitionTableTested (a : Alert) : Prop :=
  a.transitionTableTested = true

/-- 要求。 -/
def Complies (a : Alert) : Prop :=
  Applies a -> TransitionRule a /\ DiscreteEventNoted a /\ ConsumerNamed a /\ TransitionTableTested a

/-- 同じ状態の再評価で出し直すアラートは、要求を満たさない (2026-07-30 の 93 件の stuck がこれ)。 -/
theorem reemitting_a_persisting_state_fails :
    Not (Complies { occasion := Occasion.sameState, fired := true, consumerNamed := true,
                    transitionTableTested := true, discreteEvent := false,
                    notedAtEmittingSite := false, criticalRepeatJustified := false }) := by
  intro h
  cases (h trivial).1 rfl rfl

/-- 初めての越えで黙るアラートも、要求を満たさない。 -/
theorem silent_first_crossing_fails :
    Not (Complies { occasion := Occasion.firstCrossing, fired := false, consumerNamed := true,
                    transitionTableTested := true, discreteEvent := false,
                    notedAtEmittingSite := false, criticalRepeatJustified := false }) := by
  intro h
  cases (h trivial).1 rfl rfl

/-- 出す場所に書かずに離散の出来事として毎回出すアラートは、要求を満たさない。 -/
theorem unnoted_discrete_event_fails :
    Not (Complies { occasion := Occasion.sameState, fired := true, consumerNamed := true,
                    transitionTableTested := true, discreteEvent := true,
                    notedAtEmittingSite := false, criticalRepeatJustified := false }) := by
  intro h
  cases (h trivial).2.1 rfl

/-- 遷移の表をテストで留めていないアラートも、要求を満たさない。 -/
theorem untested_transition_table_fails :
    Not (Complies { occasion := Occasion.tierCrossed, fired := true, consumerNamed := true,
                    transitionTableTested := false, discreteEvent := false,
                    notedAtEmittingSite := false, criticalRepeatJustified := false }) := by
  intro h
  cases (h trivial).2.2.2

/-- 要求を満たす例。log_volume_alarm が 1 GB の段階を越えたときに、名指した消費者へ 1 回出し、
    遷移の表はテストで留めてある。 -/
theorem tier_crossing_complies :
    Complies { occasion := Occasion.tierCrossed, fired := true, consumerNamed := true,
               transitionTableTested := true, discreteEvent := false,
               notedAtEmittingSite := false, criticalRepeatJustified := false } := by
  intro _
  refine And.intro (fun _ _ => rfl) (And.intro ?_ (And.intro rfl rfl))
  intro e
  cases e

/-- 要求を満たす例。失敗したバックアップは離散の出来事として毎回出し、出す場所にそう書いてある。 -/
theorem noted_failed_backup_complies :
    Complies { occasion := Occasion.sameState, fired := true, consumerNamed := true,
               transitionTableTested := true, discreteEvent := true,
               notedAtEmittingSite := true, criticalRepeatJustified := false } := by
  intro _
  refine And.intro ?_ (And.intro (fun _ => rfl) (And.intro rfl rfl))
  intro e
  cases e

end AlertsFireOnTransitions
