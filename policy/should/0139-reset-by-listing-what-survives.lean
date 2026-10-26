/-
  should/0139-reset-by-listing-what-survives の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。きれいな状態を保証する境界
  (リセット) を構造体で表し、生き残りの一覧として書く要求を Prop にする。
-/
namespace ResetByListingWhatSurvives

/-- 次の占有者にきれいな状態を保証する境界 (測定の試行、テストの fixture、使い回すエージェントの席、
作り直す作業ディレクトリ、キャッシュの世代) の 1 つの実装。 -/
structure Reset where
  /-- その境界がきれいな状態を保証しているか。 -/
  promisesCleanSlate : Bool
  /-- 状態を持つディレクトリごと消して作り直す (消す物を列挙するのではなく) か。 -/
  sweepsWholeRegion : Bool
  /-- 作り直した後に戻すのが、名前の付いた生き残りの一覧の項目だけか。 -/
  restoresOnlySurvivors : Bool
  /-- 生き残りの一覧がリセットの場所そのものに 1 行 1 項目で書かれているか。 -/
  survivorListAtReset : Bool
  /-- 一覧の各項目に、境界を越えてよい理由が 1 行ずつ付いているか。 -/
  everySurvivorHasReason : Bool
  /-- 新しい状態の置き場が、既定で掃除される領域の中に落ちるか。 -/
  newStateLandsInsideByDefault : Bool
  /-- 越えなければならない項目に、それ専用のディレクトリを与えているか。 -/
  survivorsInOwnRegion : Bool
  /-- 規則に無いファイルを置いてリセットを走らせ、消えたことを検証するテストで境界を留めているか。 -/
  pinnedByTest : Bool
  /-- そのテストを、列挙するリセットに対して走らせて落ちることで証明した (should/0137) か。 -/
  pinTestProvedAgainstEnumeratingReset : Bool
  /-- 共有の作業場で試行を繰り返すか。 -/
  trialsRepeatInSharedWorkspace : Bool
  /-- 繰り返す前に、その根の下への書き込みを全て列挙して生き残りの一覧と突き合わせたか。 -/
  writesListedAgainstSurvivors : Bool

/-- 適用のテスト。きれいな状態を保証する境界。 -/
def Applies (r : Reset) : Prop :=
  r.promisesCleanSlate = true

/-- 領域ごと消して、生き残りだけを戻す。 -/
def SweepAndRebuild (r : Reset) : Prop :=
  r.sweepsWholeRegion = true /\ r.restoresOnlySurvivors = true

/-- 生き残りの一覧はリセットの場所に書き、各項目に理由を付ける。 -/
def SurvivorListWritten (r : Reset) : Prop :=
  r.survivorListAtReset = true /\ r.everySurvivorHasReason = true

/-- 新しい状態は既定で掃除される領域の中に落ち、越える項目は専用のディレクトリを持つ。 -/
def NewStateInside (r : Reset) : Prop :=
  r.newStateLandsInsideByDefault = true /\ r.survivorsInOwnRegion = true

/-- 境界をテストで留め、そのテストは列挙するリセットに対して証明する。 -/
def PinnedByTest (r : Reset) : Prop :=
  r.pinnedByTest = true /\ r.pinTestProvedAgainstEnumeratingReset = true

/-- 共有の作業場で試行を繰り返すなら、書き込みを列挙して一覧と突き合わせる。 -/
def WritesListed (r : Reset) : Prop :=
  r.trialsRepeatInSharedWorkspace = true -> r.writesListedAgainstSurvivors = true

/-- 要求。条項が当たるリセットは、生き残りの一覧の形で書かれている。 -/
def Complies (r : Reset) : Prop :=
  Applies r ->
    SweepAndRebuild r /\ SurvivorListWritten r /\ NewStateInside r /\ PinnedByTest r /\
    WritesListed r

/-- 消す物を列挙するリセットは、ほかを全て満たしても要求を満たさない。 -/
theorem enumerating_reset_fails
    (restores listed reasons inside own pinned proved repeats writes : Bool) :
    Not (Complies { promisesCleanSlate := true, sweepsWholeRegion := false,
                    restoresOnlySurvivors := restores, survivorListAtReset := listed,
                    everySurvivorHasReason := reasons, newStateLandsInsideByDefault := inside,
                    survivorsInOwnRegion := own, pinnedByTest := pinned,
                    pinTestProvedAgainstEnumeratingReset := proved,
                    trialsRepeatInSharedWorkspace := repeats,
                    writesListedAgainstSurvivors := writes }) := by
  intro h
  cases (h rfl).1.1

/-- 生き残りの理由が無い一覧も、要求を満たさない。 -/
theorem survivor_without_reason_fails (inside own pinned proved repeats writes : Bool) :
    Not (Complies { promisesCleanSlate := true, sweepsWholeRegion := true,
                    restoresOnlySurvivors := true, survivorListAtReset := true,
                    everySurvivorHasReason := false, newStateLandsInsideByDefault := inside,
                    survivorsInOwnRegion := own, pinnedByTest := pinned,
                    pinTestProvedAgainstEnumeratingReset := proved,
                    trialsRepeatInSharedWorkspace := repeats,
                    writesListedAgainstSurvivors := writes }) := by
  intro h
  cases (h rfl).2.1.2

/-- 要求を満たす例。エージェントの状態の領域を消して作り直し、理由付きの一覧の項目だけを戻し、
越える項目に専用のディレクトリを与え、列挙するリセットで証明したテストで留め、繰り返す前に
書き込みを突き合わせたリセット (wipe_agent を裏返した形)。 -/
theorem survivor_list_reset_complies :
    Complies { promisesCleanSlate := true, sweepsWholeRegion := true,
               restoresOnlySurvivors := true, survivorListAtReset := true,
               everySurvivorHasReason := true, newStateLandsInsideByDefault := true,
               survivorsInOwnRegion := true, pinnedByTest := true,
               pinTestProvedAgainstEnumeratingReset := true,
               trialsRepeatInSharedWorkspace := true,
               writesListedAgainstSurvivors := true } := by
  intro _
  exact And.intro (And.intro rfl rfl)
    (And.intro (And.intro rfl rfl)
      (And.intro (And.intro rfl rfl) (And.intro (And.intro rfl rfl) (fun _ => rfl))))

end ResetByListingWhatSurvives
