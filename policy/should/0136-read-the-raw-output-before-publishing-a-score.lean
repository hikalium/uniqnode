/-
  should/0136-read-the-raw-output-before-publishing-a-score
  の Lean 版。条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。対象ごとの点数の
  公表を構造体で表し、公表の前に済ませる事柄を Prop にする。
-/
namespace ReadTheRawOutputBeforePublishingAScore

/-- 対象ごとの点数の 1 回の公表 (ctl bench でのモデルの比較、バックエンドの比較、A/B の系列、
「X は Y より良い」と読まれる表)。 -/
structure Publication where
  /-- ハーネスが制御しない対象を順位付けする測定か。 -/
  ranksUncontrolledSubjects : Bool
  /-- 点数の低い対象の、集計ではなく実際に生成された本文を読んだか。 -/
  lowScorersRawOutputRead : Bool
  /-- 「測った」と「推測した」を分ける信号 (reach_shell) を行に持つか。 -/
  carriesMeasuredSignal : Bool
  /-- ハーネス側の拒否の数 (parse_failures) を行に持つか。 -/
  carriesRejectionCount : Bool
  /-- タスクに要る capability が試行に全て与えられている (有効な道具は role と grants の積) ことを検証したか。 -/
  permissionsVerified : Bool
  /-- 対象のあいだで共有されるもの (バックエンドのホスト、機械、時間帯) で走らせを分けたか。 -/
  splitBySharedFactor : Bool
  /-- 判定だけでなく生成物そのものを結果の行に残したか。 -/
  artefactKeptInRow : Bool
  /-- 分母と区間を述べたか。 -/
  denominatorAndIntervalStated : Bool

/-- 適用のテスト。ハーネスが制御しない対象を順位付けする測定の公表。 -/
def Applies (p : Publication) : Prop :=
  p.ranksUncontrolledSubjects = true

/-- 低い点数の裏の生の出力を読む。 -/
def RawOutputRead (p : Publication) : Prop :=
  p.lowScorersRawOutputRead = true

/-- 測定と推測を分ける信号と、ハーネス側の拒否の数を持つ。 -/
def SeparatingSignals (p : Publication) : Prop :=
  p.carriesMeasuredSignal = true /\ p.carriesRejectionCount = true

/-- 要る capability が試行に与えられていることを検証する。 -/
def PermissionsVerified (p : Publication) : Prop :=
  p.permissionsVerified = true

/-- 共有される要因で分ける。 -/
def SplitByShared (p : Publication) : Prop :=
  p.splitBySharedFactor = true

/-- 生成物を結果の行に残す。 -/
def ArtefactKept (p : Publication) : Prop :=
  p.artefactKeptInRow = true

/-- 分母と区間を述べる。 -/
def DenominatorStated (p : Publication) : Prop :=
  p.denominatorAndIntervalStated = true

/-- 要求。条項が当たる公表は、上の全てを済ませている。 -/
def Complies (p : Publication) : Prop :=
  Applies p ->
    RawOutputRead p /\ SeparatingSignals p /\ PermissionsVerified p /\ SplitByShared p /\
    ArtefactKept p /\ DenominatorStated p

/-- 集計だけを見て生の出力を読まずに公表した点数は、要求を満たさない。 -/
theorem aggregate_only_fails (measured rejected perms split kept denom : Bool) :
    Not (Complies { ranksUncontrolledSubjects := true, lowScorersRawOutputRead := false,
                    carriesMeasuredSignal := measured, carriesRejectionCount := rejected,
                    permissionsVerified := perms, splitBySharedFactor := split,
                    artefactKeptInRow := kept, denominatorAndIntervalStated := denom }) := by
  intro h
  cases (h rfl).1

/-- 合格率だけを示し、読み手が監査できない報告も要求を満たさない。 -/
theorem pass_rate_only_fails (raw perms split kept denom : Bool) :
    Not (Complies { ranksUncontrolledSubjects := true, lowScorersRawOutputRead := raw,
                    carriesMeasuredSignal := false, carriesRejectionCount := false,
                    permissionsVerified := perms, splitBySharedFactor := split,
                    artefactKeptInRow := kept, denominatorAndIntervalStated := denom }) := by
  intro h
  cases (h rfl).2.1.1

/-- 要求を満たす例。生の出力を読み、分ける信号と生成物を行に残し、分母を添えた比較の表。 -/
theorem audited_comparison_complies :
    Complies { ranksUncontrolledSubjects := true, lowScorersRawOutputRead := true,
               carriesMeasuredSignal := true, carriesRejectionCount := true,
               permissionsVerified := true, splitBySharedFactor := true,
               artefactKeptInRow := true, denominatorAndIntervalStated := true } := by
  intro _
  exact And.intro rfl (And.intro (And.intro rfl rfl)
    (And.intro rfl (And.intro rfl (And.intro rfl rfl))))

end ReadTheRawOutputBeforePublishingAScore
