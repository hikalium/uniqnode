/-
  should/0129-alerts-fire-on-transitions の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。アラートを出す機会を型で表し、
  遷移のときだけ出すという要求を Prop にする。段階の大きさや間隔の値 (log_volume_alarm の 1 GB、6 時間、
  10 パーセント、24 時間) は出す側の定数にあり、ここでは扱わない。
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
  /-- 悪い状態が続く中で、思い出させる間隔が過ぎた。 -/
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
  /-- 起きるたびに独立して対処する離散の出来事か (失敗したバックアップ、セキュリティの拒否)。 -/
  discreteEvent : Bool

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

/-- 適用のテスト。状態を監視する全てのアラートの経路。離散の出来事は逸脱として外に置く。 -/
def Applies (a : Alert) : Prop :=
  a.discreteEvent = false

/-- 遷移のときに出し、遷移でないときは黙る。 -/
def FiresExactlyOnTransitions (a : Alert) : Prop :=
  a.fired = a.occasion.isTransition

/-- 消費者を設計時に名指す。 -/
def ConsumerNamed (a : Alert) : Prop :=
  a.consumerNamed = true

/-- 要求。 -/
def Complies (a : Alert) : Prop :=
  Applies a -> FiresExactlyOnTransitions a /\ ConsumerNamed a

/-- 同じ状態の再評価で出し直すアラートは、要求を満たさない (2026-07-30 の 93 件の stuck がこれ)。 -/
theorem reemitting_a_persisting_state_fails :
    Not (Complies { occasion := Occasion.sameState, fired := true, consumerNamed := true,
                    discreteEvent := false }) := by
  intro h
  cases (h rfl).1

/-- クールダウンの中で割合の再越えを出すアラートも、要求を満たさない。 -/
theorem rate_inside_cooldown_fails :
    Not (Complies { occasion := Occasion.rateInsideCooldown, fired := true,
                    consumerNamed := true, discreteEvent := false }) := by
  intro h
  cases (h rfl).1

/-- 初めての越えで黙るアラートも、要求を満たさない。 -/
theorem silent_first_crossing_fails :
    Not (Complies { occasion := Occasion.firstCrossing, fired := false, consumerNamed := true,
                    discreteEvent := false }) := by
  intro h
  cases (h rfl).1

/-- 要求を満たす例。log_volume_alarm が 1 GB の段階を越えたときに、名指した消費者へ 1 回出す。 -/
theorem tier_crossing_complies :
    Complies { occasion := Occasion.tierCrossed, fired := true, consumerNamed := true,
               discreteEvent := false } := by
  intro _
  exact And.intro rfl rfl

/-- 要求を満たす例。同じ状態のままの再評価では黙る。 -/
theorem silent_same_state_complies :
    Complies { occasion := Occasion.sameState, fired := false, consumerNamed := true,
               discreteEvent := false } := by
  intro _
  exact And.intro rfl rfl

end AlertsFireOnTransitions
