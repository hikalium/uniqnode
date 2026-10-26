/-
  should/0132-attribute-the-cost-before-optimizing の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  性能の変更 1 つを主題にして、コストの帰属と測定の交絡の制御を要求として Prop にする。
-/
namespace AttributeTheCostBeforeOptimizing

/-- 性能 (遅延、処理量、資源の使用) に関わる 1 つの変更と、その根拠の測定。 -/
structure PerformanceChange where
  /-- 性能の仕事か。 -/
  isPerformanceWork : Bool
  /-- ハーネス、viewer、ホストの道具のどれかの中の仕事か。 -/
  inHarnessViewerOrHostTooling : Bool
  /-- 直しを書く前に、疑った機構を名指して直接測り、コストを帰属させたか。 -/
  costAttributedBeforeFix : Bool
  /-- 主な根拠の測定を稼働系の上で取ったか。 -/
  primaryFromLiveSystem : Bool
  /-- その測定の散らばりが、追っている効果より広いか。 -/
  spreadWiderThanEffect : Bool
  /-- 利用者にとって冷えた状態を模しているか。 -/
  modelsColdSituation : Bool
  /-- 標本の間隔を空けて、測定でも冷えた状態にしたか。 -/
  samplesSpacedCold : Bool
  /-- 数値がどちらの状態 (温かい、冷えた) のものかを書いたか。 -/
  regimeStated : Bool
  /-- 主張がどの測定に基づくかを書いたか。 -/
  measurementNamed : Bool

/-- 適用のテスト。ハーネス、viewer、ホストの道具の中の性能の仕事。 -/
def Applies (c : PerformanceChange) : Prop :=
  c.isPerformanceWork = true /\ c.inHarnessViewerOrHostTooling = true

/-- 先に帰属させる。直しはコストを帰属させた後に選ぶ。 -/
def AttributedFirst (c : PerformanceChange) : Prop :=
  c.costAttributedBeforeFix = true

/-- 競合を制御する。稼働系の測定は、散らばりが効果より広ければ主な根拠にできない。 -/
def ContentionControlled (c : PerformanceChange) : Prop :=
  c.primaryFromLiveSystem = true -> c.spreadWiderThanEffect = false

/-- キャッシュの温まりを制御する。冷えた状態を模すなら標本を空け、どの状態の数値かを書く。 -/
def CacheWarmthControlled (c : PerformanceChange) : Prop :=
  (c.modelsColdSituation = true -> c.samplesSpacedCold = true) /\ c.regimeStated = true

/-- どの測定に基づく主張かを書く。 -/
def MeasurementNamed (c : PerformanceChange) : Prop :=
  c.measurementNamed = true

/-- 要求。 -/
def Complies (c : PerformanceChange) : Prop :=
  Applies c -> AttributedFirst c /\ ContentionControlled c /\ CacheWarmthControlled c /\
    MeasurementNamed c

/-- もっともらしい原因を測らずに直した変更は、要求を満たさない。 -/
theorem fix_without_attribution_fails :
    Not (Complies { isPerformanceWork := true, inHarnessViewerOrHostTooling := true,
                    costAttributedBeforeFix := false,
                    primaryFromLiveSystem := false, spreadWiderThanEffect := false,
                    modelsColdSituation := false, samplesSpacedCold := false,
                    regimeStated := true, measurementNamed := true }) := by
  intro h
  have e : false = true := (h (And.intro rfl rfl)).1
  cases e

/-- 散らばりが効果より広い稼働系の測定を主な根拠にした変更は、要求を満たさない。 -/
theorem noisy_live_probe_as_primary_fails :
    Not (Complies { isPerformanceWork := true, inHarnessViewerOrHostTooling := true,
                    costAttributedBeforeFix := true,
                    primaryFromLiveSystem := true, spreadWiderThanEffect := true,
                    modelsColdSituation := false, samplesSpacedCold := false,
                    regimeStated := true, measurementNamed := true }) := by
  intro h
  have e : true = false := (h (And.intro rfl rfl)).2.1 rfl
  cases e

/-- 冷えた状態を模すのに、続けて再読み込みして温かい数値を取った変更は、要求を満たさない。 -/
theorem warm_samples_for_cold_situation_fails :
    Not (Complies { isPerformanceWork := true, inHarnessViewerOrHostTooling := true,
                    costAttributedBeforeFix := true,
                    primaryFromLiveSystem := false, spreadWiderThanEffect := false,
                    modelsColdSituation := true, samplesSpacedCold := false,
                    regimeStated := true, measurementNamed := true }) := by
  intro h
  have e : false = true := (h (And.intro rfl rfl)).2.2.1.1 rfl
  cases e

/-- 要求を満たす例。strace で項目ごとの stat に帰属させ、隔離した経路で冷えた状態を測った変更。 -/
theorem strace_attributed_change_complies :
    Complies { isPerformanceWork := true, inHarnessViewerOrHostTooling := true,
               costAttributedBeforeFix := true,
               primaryFromLiveSystem := false, spreadWiderThanEffect := false,
               modelsColdSituation := true, samplesSpacedCold := true,
               regimeStated := true, measurementNamed := true } := by
  intro _
  exact And.intro rfl (And.intro (fun e => by cases e)
    (And.intro (And.intro (fun _ => rfl) rfl) rfl))

end AttributeTheCostBeforeOptimizing
