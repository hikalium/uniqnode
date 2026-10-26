/-
  must/0014-update-tests-with-contract の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。インターフェースの契約 (プロトコル、
  データ形式、API の形、検証と受け入れの意味) への 1 つの変更を型で表し、適用のテストと要求を Prop にする。
-/
namespace UpdateTestsWithContract

/-- コードへの 1 つの変更と、それが残したテストの状態。 -/
structure Change where
  /-- インターフェースの契約を変える変更か。 -/
  changesContract : Bool
  /-- その契約を使う全てのテストとフィクスチャを、同じ変更の中で更新したか。 -/
  testsUpdatedInSameChange : Bool
  /-- スイートが失敗したまま残っているか。 -/
  suiteLeftFailing : Bool
  /-- 合格しながら、古い契約をまだ試しているテストやフィクスチャがあるか。 -/
  suiteExercisesOldContract : Bool

/-- 適用のテスト。インターフェースの契約を変える変更。 -/
def Applies (c : Change) : Prop :=
  c.changesContract = true

/-- 契約を使うテストとフィクスチャを同じ変更で更新する。 -/
def TestsTravelWithTheContract (c : Change) : Prop :=
  c.testsUpdatedInSameChange = true

/-- スイートを失敗のまま残さない。直すか、変更を戻す。赤い回帰スイートは何も検出しない。 -/
def SuiteNotLeftFailing (c : Change) : Prop :=
  c.suiteLeftFailing = false

/-- 合格しながら引退した契約を試すスイートも残さない。 -/
def SuiteExercisesNewContractOnly (c : Change) : Prop :=
  c.suiteExercisesOldContract = false

/-- 要求。 -/
def Complies (c : Change) : Prop :=
  Applies c -> TestsTravelWithTheContract c /\ SuiteNotLeftFailing c /\
    SuiteExercisesNewContractOnly c

/-- 2026-07 の実例。入力の契約を締めたのに決定的なテストのスイートを更新せず、2 週間赤いまま残した変更は、
    要求を満たさない。 -/
theorem red_suite_left_behind_fails :
    Not (Complies { changesContract := true, testsUpdatedInSameChange := false,
                    suiteLeftFailing := true, suiteExercisesOldContract := false }) := by
  intro h
  cases (h rfl).2.1

/-- PASS のまま古い契約を試し続けるスイートを残した変更も、要求を満たさない。 -/
theorem green_suite_on_retired_contract_fails :
    Not (Complies { changesContract := true, testsUpdatedInSameChange := false,
                    suiteLeftFailing := false, suiteExercisesOldContract := true }) := by
  intro h
  cases (h rfl).2.2

/-- 要求を満たす例。契約を変え、同じ変更でテストとフィクスチャを新しい契約に揃えた。 -/
theorem contract_change_with_tests_complies :
    Complies { changesContract := true, testsUpdatedInSameChange := true,
               suiteLeftFailing := false, suiteExercisesOldContract := false } := by
  intro _
  exact And.intro rfl (And.intro rfl rfl)

/-- 契約を変えない変更には当たらない。 -/
theorem non_contract_change_is_out_of_scope :
    Complies { changesContract := false, testsUpdatedInSameChange := false,
               suiteLeftFailing := false, suiteExercisesOldContract := false } := by
  intro h
  cases h

end UpdateTestsWithContract
