/-
  should/0138-probe-with-the-request-the-production-caller-builds の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  稼働中のサービスを判定するために手で出した要求と、そこから導いた所見を構造にし、
  要求の組み立て (llm_payload_for などの組み立て役)、所見の書き方、再現の順序を要求として表す。
-/
namespace ProbeWithTheRequestTheProductionCallerBuilds

/-- 稼働中のサービスを判定するための診断の要求と、そこから導いた所見。 -/
structure Probe where
  /-- 稼働中のサービスが健全かを判定するために出した要求か。 -/
  judgesLiveService : Bool
  /-- 組み立て役そのものを直している最中に出した要求か (条項が認める例外)。 -/
  builderUnderRepair : Bool
  /-- 艦隊の組み立て役 (llm_payload_for、stream_payload_for、say_ops) を通して出したか。 -/
  throughBuilder : Bool
  /-- 手書きの要求で、組み立て役を開いて全ての欄 (think、keep_alive、num_ctx、max_tokens) を写し、
      欄ごとに差分を取ったか。 -/
  fieldsCopiedFromBuilder : Bool
  /-- 各値を、艦隊が取る元 (resident_context_tokens など) から取ったか。 -/
  valuesFromProductionSource : Bool
  /-- 所見が、通した組み立て役か呼び出し側を名指しているか。 -/
  findingNamesBuilder : Bool
  /-- 所見が、写せなかった欄を列挙しているか。 -/
  findingListsOmittedFields : Bool
  /-- 症状を本番の呼び出し側 (seat の 1 往復、bench の試行、チャットの 1 往復) で再現したか。 -/
  reproducedThroughProductionCaller : Bool
  /-- 症状を艦隊の故障として記録したか。 -/
  recordedAsFleetFailure : Bool
  /-- 高くつく帰結 (sudo の依頼、ロールバック、撤回の公表、インシデント) を起こしたか。 -/
  expensiveConsequenceTaken : Bool
  /-- 本物の欠陥を見つけた要求か。 -/
  foundRealDefect : Bool
  /-- 組み立て役を共有する計器 (promptprobe のように) として木に入れたか。 -/
  landedAsInstrument : Bool

/-- 適用のテスト。稼働中のサービスを判定する手出しの要求。組み立て役の修理中は例外。 -/
def Applies (p : Probe) : Prop :=
  p.judgesLiveService = true /\ p.builderUnderRepair = false

/-- 要求は組み立て役を通すか、手書きなら全ての欄と値を艦隊の元から写す。 -/
def RequestFollowsBuilder (p : Probe) : Prop :=
  p.throughBuilder = true \/
    (p.fieldsCopiedFromBuilder = true /\ p.valuesFromProductionSource = true)

/-- 所見は組み立て役を名指し、写せなかった欄を列挙する。 -/
def FindingNamesBuilder (p : Probe) : Prop :=
  p.findingNamesBuilder = true /\ p.findingListsOmittedFields = true

/-- 艦隊の故障として記録する前に、本番の呼び出し側で再現する。 -/
def ReproducedBeforeRecording (p : Probe) : Prop :=
  p.recordedAsFleetFailure = true -> p.reproducedThroughProductionCaller = true

/-- 高くつく帰結は、本番の経路が同じ失敗を見せた後に限る。 -/
def ReproducedBeforeConsequence (p : Probe) : Prop :=
  p.expensiveConsequenceTaken = true -> p.reproducedThroughProductionCaller = true

/-- 本物の欠陥を見つけた要求は、組み立て役を共有する計器として木に入れる。 -/
def DefectProbeLands (p : Probe) : Prop :=
  p.foundRealDefect = true -> p.landedAsInstrument = true

/-- 要求。条項が当たる診断の要求は、上の 5 つを全て満たす。 -/
def Complies (p : Probe) : Prop :=
  Applies p ->
    RequestFollowsBuilder p /\ FindingNamesBuilder p /\ ReproducedBeforeRecording p /\
      ReproducedBeforeConsequence p /\ DefectProbeLands p

/-- 条項が止めたい形 (2026-08-10 の orion)。num_ctx を省いた curl だけを根拠に、再現せずに
    ロールバックを頼む。 -/
def handWrittenRollback : Probe :=
  { judgesLiveService := true, builderUnderRepair := false, throughBuilder := false,
    fieldsCopiedFromBuilder := false, valuesFromProductionSource := false,
    findingNamesBuilder := false, findingListsOmittedFields := false,
    reproducedThroughProductionCaller := false, recordedAsFleetFailure := true,
    expensiveConsequenceTaken := true, foundRealDefect := false, landedAsInstrument := false }

/-- 欄を省いた手書きの要求だけで艦隊の故障を記録した診断は、要求を満たさない。 -/
theorem hand_written_rollback_fails : Not (Complies handWrittenRollback) := by
  intro h
  have applies : Applies handWrittenRollback := And.intro rfl rfl
  have e : false = true := (h applies).2.2.1 rfl
  cases e

/-- 要求を満たす例。prompt-probe のように組み立て役を通し、所見に組み立て役を書き、
    seat の 1 往復で再現してから記録する。 -/
theorem builder_probe_complies :
    Complies { judgesLiveService := true, builderUnderRepair := false, throughBuilder := true,
               fieldsCopiedFromBuilder := false, valuesFromProductionSource := true,
               findingNamesBuilder := true, findingListsOmittedFields := true,
               reproducedThroughProductionCaller := true, recordedAsFleetFailure := true,
               expensiveConsequenceTaken := false, foundRealDefect := true,
               landedAsInstrument := true } := by
  intro _
  exact And.intro (Or.inl rfl)
    (And.intro (And.intro rfl rfl)
      (And.intro (fun _ => rfl) (And.intro (fun _ => rfl) (fun _ => rfl))))

end ProbeWithTheRequestTheProductionCallerBuilds
