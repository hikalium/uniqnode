/-
  should/0103-diagnose-by-isolation の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。失敗する振る舞いの 1 回の診断を
  構造体で表し、推測ではなく切り分けで進める要求を Prop にする。
-/
namespace DiagnoseByIsolation

/-- システムの失敗する振る舞いの 1 回の診断 (人、claude-remote、エージェントのどれが行うかを問わない)。 -/
structure Diagnosis where
  /-- システムの中で失敗するか誤って振る舞うものを診断しているか。 -/
  debugsFailure : Bool
  /-- 既知の良い最小の構成から始めて、1 段階ごとに 1 つの要因だけを足すか除くか。 -/
  oneVariablePerStep : Bool
  /-- 選択肢やフラグやプロパティの一式をまとめて入れ替えたか。 -/
  swapsWholeSetAtOnce : Bool
  /-- 動く部分の多い端から端までの走らせより、決定的で最小の再現 (疑う点だけを動かす probe) を
  選んだか。 -/
  prefersMinimalReproduction : Bool
  /-- 何かが未対応だ、環境が壊れていると結論したか。 -/
  concludesUnsupported : Bool
  /-- その結論の前に、上の切り分けの段階を使い切ったか。 -/
  isolationExhausted : Bool

/-- 適用のテスト。失敗する振る舞いの診断。 -/
def Applies (d : Diagnosis) : Prop :=
  d.debugsFailure = true

/-- 1 度に 1 つの変数を変え、一式をまとめて入れ替えない。 -/
def OneVariableAtATime (d : Diagnosis) : Prop :=
  d.oneVariablePerStep = true /\ d.swapsWholeSetAtOnce = false

/-- 最小の再現を選ぶ。 -/
def MinimalReproduction (d : Diagnosis) : Prop :=
  d.prefersMinimalReproduction = true

/-- 「自分の設定が間違っている」を既定にし、切り分けを使い切る前に未対応と結論しない。 -/
def ConfigBeforeEnvironment (d : Diagnosis) : Prop :=
  d.concludesUnsupported = true -> d.isolationExhausted = true

/-- 要求。条項が当たる診断は、切り分けで進む。 -/
def Complies (d : Diagnosis) : Prop :=
  Applies d -> OneVariableAtATime d /\ MinimalReproduction d /\ ConfigBeforeEnvironment d

/-- 一式をまとめて入れ替える診断は、原因を混ぜるので要求を満たさない。 -/
theorem swapping_whole_set_fails (one minimal unsupported exhausted : Bool) :
    Not (Complies { debugsFailure := true, oneVariablePerStep := one,
                    swapsWholeSetAtOnce := true, prefersMinimalReproduction := minimal,
                    concludesUnsupported := unsupported, isolationExhausted := exhausted }) := by
  intro h
  cases (h rfl).1.2

/-- 切り分けを使い切る前に「環境が壊れている」と結論する診断は、要求を満たさない。 -/
theorem premature_unsupported_fails :
    Not (Complies { debugsFailure := true, oneVariablePerStep := true,
                    swapsWholeSetAtOnce := false, prefersMinimalReproduction := true,
                    concludesUnsupported := true, isolationExhausted := false }) := by
  intro h
  cases (h rfl).2.2 rfl

/-- 要求を満たす例。ctl probe で systemd-run の性質を 1 つずつ試し、自分の設定の誤りを見つけた診断。 -/
theorem isolating_diagnosis_complies :
    Complies { debugsFailure := true, oneVariablePerStep := true,
               swapsWholeSetAtOnce := false, prefersMinimalReproduction := true,
               concludesUnsupported := false, isolationExhausted := true } := by
  intro _
  exact And.intro (And.intro rfl rfl) (And.intro rfl (fun _ => rfl))

end DiagnoseByIsolation
