/-
  should/0140-store-a-ledger-with-the-work-it-records の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。台帳とその主題の寿命の関係を型で表し、
  適用のテストと要求を Prop にする。
-/
namespace StoreALedgerWithTheWorkItRecords

/-- 台帳と主題が、どのリセットで消えるかの関係。 -/
inductive Lifetime where
  /-- 同じ領域にあり、同じ掃除で一緒に消えるか、一緒に生き延びる。 -/
  | together
  /-- 台帳が先に消える。主題が毎回新しいものとして届く (作業の繰り返し)。 -/
  | ledgerDiesFirst
  /-- 主題が先に消える。残った台帳が新しい主題を黙って飛ばす (作業の取りこぼし)。 -/
  | subjectDiesFirst

/-- 1 つの台帳 (配達済みのマーク、重複除去の集合、カーソル、done ファイルなど)。 -/
structure Ledger where
  /-- 台帳と主題の寿命の関係。 -/
  lifetime : Lifetime
  /-- 領域の境界を越えるとき、どのリセットを生き延びる想定かとその理由が、台帳の置き場所に書いてあるか。 -/
  reasonAtSite : Bool
  /-- 境界をテストで留めてあるか (1 件を処理済みにし、本番のリセットを走らせ、二度目が無いことを検証する)。 -/
  pinnedByTest : Bool

/-- 適用のテスト。全ての台帳に当たる。 -/
def Applies (_l : Ledger) : Prop := True

/-- 台帳は主題と同じ領域に置く。越えるなら、その理由を台帳の置き場所に書く。 -/
def Placement (l : Ledger) : Prop :=
  l.lifetime = Lifetime.together \/ l.reasonAtSite = true

/-- 境界はテストで留める (should/0137 に従い、今の置き場所に対して先に走らせて証明する)。 -/
def Pinned (l : Ledger) : Prop :=
  l.pinnedByTest = true

/-- 要求。置き場所の規則と、テストによる固定。 -/
def Complies (l : Ledger) : Prop :=
  Applies l -> Placement l /\ Pinned l

/-- 寿命の食い違いが買った失敗。together は失敗を買わない。 -/
def Lifetime.repeatsWork : Lifetime -> Bool
  | .together => false
  | .ledgerDiesFirst => true
  | .subjectDiesFirst => false

/-- 測った実例。DELIVERED_LEDGER が work/ にあって毎回消え、主題は state/pending/ に残り、
    理由も書いていない。要求を満たさない。 -/
theorem ledger_dying_first_without_reason_fails :
    Not (Complies { lifetime := Lifetime.ledgerDiesFirst, reasonAtSite := false,
                    pinnedByTest := true }) := by
  intro h
  have p : Lifetime.ledgerDiesFirst = Lifetime.together \/ false = true := (h trivial).1
  cases p with
  | inl e => cases e
  | inr e => cases e

/-- 主題が先に消える置き方も、理由が無ければ要求を満たさない。 -/
theorem subject_dying_first_without_reason_fails :
    Not (Complies { lifetime := Lifetime.subjectDiesFirst, reasonAtSite := false,
                    pinnedByTest := true }) := by
  intro h
  have p : Lifetime.subjectDiesFirst = Lifetime.together \/ false = true := (h trivial).1
  cases p with
  | inl e => cases e
  | inr e => cases e

/-- 要求を満たす例。配達済みのマークを主題と同じ領域に置き、テストで境界を留めた台帳。 -/
theorem same_region_ledger_complies :
    Complies { lifetime := Lifetime.together, reasonAtSite := false, pinnedByTest := true } := by
  intro _
  exact And.intro (Or.inl rfl) rfl

/-- 要求を満たす例。追記だけのログへのカーソルは、キャッシュの世代を越えて生きる理由を書いて越える。 -/
theorem reasoned_crossing_complies :
    Complies { lifetime := Lifetime.subjectDiesFirst, reasonAtSite := true,
               pinnedByTest := true } := by
  intro _
  exact And.intro (Or.inr rfl) rfl

/-- 台帳が先に消える置き方は、作業の繰り返しを買う。 -/
theorem ledger_dying_first_repeats_work :
    Lifetime.repeatsWork Lifetime.ledgerDiesFirst = true := rfl

end StoreALedgerWithTheWorkItRecords
