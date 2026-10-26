/-
  should/0130-new-state-is-the-last-resort の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  異常の扱いの案を構造にし、新しい状態を足すのは 4 つの安い道を順に検討して足りないと示した
  後に限り、そのときは遷移と消費側を名指すことを要求として表す。
-/
namespace NewStateIsTheLastResort

/-- 状態機械 (タスクとエピソードの status、エージェントの状態、条件の次元、門の判定) の
    異常を扱う案。 -/
structure Proposal where
  /-- 状態機械の異常を扱う案か。 -/
  handlesAnomalyInStateMachine : Bool
  /-- 新しい状態か status を足す案か。 -/
  addsNewState : Bool
  /-- 入力の質を直して異常が出なくなるかを検討し、足りないと示したか。 -/
  inputFixShownInsufficient : Bool
  /-- 消費側で吸収する (ならす、言い換える、既存の場合として扱う) かを検討し、足りないと示したか。 -/
  absorbShownInsufficient : Bool
  /-- 測って観察するだけで済むかを検討し、足りないと示したか。 -/
  observeShownInsufficient : Bool
  /-- 同じ意味の既存の状態を使い回せるかを検討し、足りないと示したか。 -/
  reuseShownInsufficient : Bool
  /-- どの道がどう足りなかったかを書いたか。 -/
  failedRouteNamed : Bool
  /-- 足す遷移を名指しているか。 -/
  namesTransitions : Bool
  /-- 触る消費側を名指しているか。 -/
  namesConsumers : Bool

/-- 適用のテスト。状態機械の異常を扱う案。 -/
def Applies (p : Proposal) : Prop :=
  p.handlesAnomalyInStateMachine = true

/-- 4 つの安い道を全て検討し、足りないと示した。 -/
def CheaperRoutesExhausted (p : Proposal) : Prop :=
  p.inputFixShownInsufficient = true /\ p.absorbShownInsufficient = true /\
    p.observeShownInsufficient = true /\ p.reuseShownInsufficient = true

/-- 新しい状態を足す案は、安い道を尽くし、失敗した道と遷移と消費側を名指す。 -/
def NewStateJustified (p : Proposal) : Prop :=
  p.addsNewState = true ->
    CheaperRoutesExhausted p /\ p.failedRouteNamed = true /\ p.namesTransitions = true /\
      p.namesConsumers = true

/-- 要求。条項が当たる案は、新しい状態を最後の手としてだけ足す。 -/
def Complies (p : Proposal) : Prop :=
  Applies p -> NewStateJustified p

/-- 条項が止めたい形 (2026-07-20)。受理された引用が 0 の witness に witness_starved の status と
    警報を足す。 -/
theorem starved_status_fails (named transitions consumers : Bool) :
    Not (Complies { handlesAnomalyInStateMachine := true, addsNewState := true,
                    inputFixShownInsufficient := false, absorbShownInsufficient := false,
                    observeShownInsufficient := false, reuseShownInsufficient := false,
                    failedRouteNamed := named, namesTransitions := transitions,
                    namesConsumers := consumers }) := by
  intro h
  have e : false = true := (h rfl rfl).1.1
  cases e

/-- 要求を満たす例。既存の abandoned の status を使い回し、新しい状態を足さない。 -/
theorem reuse_existing_complies :
    Complies { handlesAnomalyInStateMachine := true, addsNewState := false,
               inputFixShownInsufficient := true, absorbShownInsufficient := true,
               observeShownInsufficient := true, reuseShownInsufficient := false,
               failedRouteNamed := true, namesTransitions := false,
               namesConsumers := false } := by
  intro _ e
  cases e

/-- 4 つの道を尽くして失敗した道と遷移と消費側を名指した案は、新しい状態を足してよい。 -/
theorem exhausted_routes_complies :
    Complies { handlesAnomalyInStateMachine := true, addsNewState := true,
               inputFixShownInsufficient := true, absorbShownInsufficient := true,
               observeShownInsufficient := true, reuseShownInsufficient := true,
               failedRouteNamed := true, namesTransitions := true,
               namesConsumers := true } := by
  intro _ _
  exact And.intro (And.intro rfl (And.intro rfl (And.intro rfl rfl)))
    (And.intro rfl (And.intro rfl rfl))

end NewStateIsTheLastResort
