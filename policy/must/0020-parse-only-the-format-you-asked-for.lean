/-
  must/0020-parse-only-the-format-you-asked-for の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。LLM の出力を機械が読む 1 つの
  ハーネスの場所を型で表し、適用のテストと要求を Prop にする。
-/
namespace ParseOnlyTheFormatYouAskedFor

/-- LLM の出力を機械が読むハーネスの 1 つの場所 (tool のプロトコル、judge や vision や witness の問い、
    構造化された答えの契約) と、それに対になるプロンプト。 -/
structure ParserSite where
  /-- 出力を機械が読むか。 -/
  machineReads : Bool
  /-- パーサの anchor と判別のマークの全てが、プロンプトが明示的に求めた字句と構造か。 -/
  anchorsRequestedByPrompt : Bool
  /-- 種類の違うフィールドを、文体の慣習を区切りにして 1 つの自由な文字列に多重化しているか。 -/
  multiplexedByConvention : Bool
  /-- テストのフィクスチャが、プロンプトが求めていない形式で書かれているか。 -/
  fixtureUsesUnrequestedFormat : Bool
  /-- 契約を満たしつつ任意の飾りを 1 つも持たない入力のテストがあるか。 -/
  hasUnstyledConformingTest : Bool

/-- 適用のテスト。LLM の出力を機械が読む全ての場所。 -/
def Applies (p : ParserSite) : Prop :=
  p.machineReads = true

/-- anchor と判別のマークは、プロンプトが明示的に求めた字句と構造に限る。 -/
def AnchorsFromPrompt (p : ParserSite) : Prop :=
  p.anchorsRequestedByPrompt = true

/-- フィールドは形式の慣習で多重化しない。構造 (anchor の字句、別の問い、別の呼び出し) で分ける。 -/
def NoConventionMultiplexing (p : ParserSite) : Prop :=
  p.multiplexedByConvention = false

/-- フィクスチャは契約に従い、癖に従わない。 -/
def FixtureFollowsContract (p : ParserSite) : Prop :=
  p.fixtureUsesUnrequestedFormat = false

/-- 契約を満たす、飾りの無い入力のテストがある。 -/
def UnstyledConformingTest (p : ParserSite) : Prop :=
  p.hasUnstyledConformingTest = true

/-- 要求。 -/
def Complies (p : ParserSite) : Prop :=
  Applies p -> AnchorsFromPrompt p /\ NoConventionMultiplexing p /\ FixtureFollowsContract p /\
    UnstyledConformingTest p

/-- looks_block の実例。プロンプトが求めていない「- 」の接頭辞で欠陥と説明を分け、フィクスチャだけが
    その接頭辞を使っていたパーサは、要求を満たさない。 -/
theorem keying_on_a_habit_fails :
    Not (Complies { machineReads := true, anchorsRequestedByPrompt := false,
                    multiplexedByConvention := true, fixtureUsesUnrequestedFormat := true,
                    hasUnstyledConformingTest := false }) := by
  intro h
  cases (h rfl).1

/-- 字句はプロンプトのものでも、飾りの無い入力のテストを欠くパーサは、要求を満たさない。 -/
theorem missing_unstyled_test_fails :
    Not (Complies { machineReads := true, anchorsRequestedByPrompt := true,
                    multiplexedByConvention := false, fixtureUsesUnrequestedFormat := false,
                    hasUnstyledConformingTest := false }) := by
  intro h
  cases (h rfl).2.2.2

/-- 要求を満たす例。ハーネスが指定した anchor でフィールドを分け、飾りの無い入力のテストを持つパーサ。 -/
theorem anchored_fields_parser_complies :
    Complies { machineReads := true, anchorsRequestedByPrompt := true,
               multiplexedByConvention := false, fixtureUsesUnrequestedFormat := false,
               hasUnstyledConformingTest := true } := by
  intro _
  exact And.intro rfl (And.intro rfl (And.intro rfl rfl))

/-- 人が読む答えには当たらない。 -/
theorem human_read_answer_is_out_of_scope :
    Complies { machineReads := false, anchorsRequestedByPrompt := false,
               multiplexedByConvention := true, fixtureUsesUnrequestedFormat := true,
               hasUnstyledConformingTest := false } := by
  intro h
  cases h

end ParseOnlyTheFormatYouAskedFor
