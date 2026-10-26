/-
  should/0111-observation-is-for-action の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  仕事の中の 1 つの局面を主題にして、3 つの状況とそれぞれに対になる行動を要求として Prop にする。
-/
namespace ObservationIsForAction

/-- 仕事の中の 1 つの局面。エージェントと操作者の全ての仕事に当たる。 -/
structure Step where
  /-- 次の決定に要るものを既に持っているか。 -/
  knowsEnoughToAct : Bool
  /-- それでも集め続けているか。 -/
  keepsGathering : Bool
  /-- 既に見たものを、見てから何も変わっていないのに、また見ようとしているか。 -/
  reexaminesUnchanged : Bool
  /-- 変化を生まない試みの連なりか、同じ計画の言い直しの中にいるか。 -/
  nothingIsChanging : Bool
  /-- 別の具体的な行動に切り替えたか、項目を締めたか、止めたか。 -/
  changesWhatItDoes : Bool

/-- 適用のテスト。全ての仕事に当たる。 -/
def Applies (_s : Step) : Prop :=
  True

/-- 動くのに足りると分かったら動く。 -/
def ActsWhenEnough (s : Step) : Prop :=
  s.knowsEnoughToAct = true -> s.keepsGathering = false

/-- 既に見たものを見直さない。 -/
def NoReexamination (s : Step) : Prop :=
  s.reexaminesUnchanged = false

/-- 何も変わらないなら、していることを変える。 -/
def ChangesWhenStuck (s : Step) : Prop :=
  s.nothingIsChanging = true -> s.changesWhatItDoes = true

/-- 要求。 -/
def Complies (s : Step) : Prop :=
  Applies s -> ActsWhenEnough s /\ NoReexamination s /\ ChangesWhenStuck s

/-- 足りているのに集め続ける局面は、要求を満たさない。 -/
theorem gathering_past_sufficiency_fails :
    Not (Complies { knowsEnoughToAct := true, keepsGathering := true,
                    reexaminesUnchanged := false, nothingIsChanging := false,
                    changesWhatItDoes := false }) := by
  intro h
  have e : true = false := (h trivial).1 rfl
  cases e

/-- 変わっていない事実をもう一度取りに行く局面は、要求を満たさない。 -/
theorem rechecking_unchanged_fact_fails :
    Not (Complies { knowsEnoughToAct := false, keepsGathering := true,
                    reexaminesUnchanged := true, nothingIsChanging := false,
                    changesWhatItDoes := false }) := by
  intro h
  have e : true = false := (h trivial).2.1
  cases e

/-- 同じ計画を言い直すだけで行動を変えない局面は、要求を満たさない。 -/
theorem restating_the_plan_fails :
    Not (Complies { knowsEnoughToAct := true, keepsGathering := false,
                    reexaminesUnchanged := false, nothingIsChanging := true,
                    changesWhatItDoes := false }) := by
  intro h
  have e : false = true := (h trivial).2.2 rfl
  cases e

/-- 要求を満たす例。足りると分かって動き、止まった試みを別の行動に切り替えた局面。 -/
theorem acting_step_complies :
    Complies { knowsEnoughToAct := true, keepsGathering := false, reexaminesUnchanged := false,
               nothingIsChanging := true, changesWhatItDoes := true } := by
  intro _
  exact And.intro (fun _ => rfl) (And.intro rfl (fun _ => rfl))

end ObservationIsForAction
