/-
  should/0114-immediate-usability-no-startup-gates の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。対話的な成果物が開いた瞬間に
  見せる画面を型で表し、主な UI の前に入口のゲートを置かない要求を Prop にする。
-/
namespace ImmediateUsabilityNoStartupGates

/-- 成果物が開いた瞬間に利用者の前にある画面。 -/
inductive Opening where
  /-- 主なインターフェースが見えて操作できる。 -/
  | primaryUi
  /-- 「Start」ボタン、splash、歓迎の interstitial のような入口のゲート。 -/
  | startGate
  /-- 法や安全のために要る確認。 -/
  | legalOrSafetyConfirmation
  /-- 壊す操作の前の守り。 -/
  | destructiveGuard
  /-- credential の入力。 -/
  | credentialEntry

/-- 1 つの対話的な成果物 (web ページ、TUI、対話的な CLI)。 -/
structure Deliverable where
  /-- 対話的な成果物か。 -/
  interactive : Bool
  /-- 開いた瞬間の画面。 -/
  opening : Opening
  /-- プラットフォームの制限が利用者の操作を本当に要する (AudioContext の自動再生、許可の
  プロンプト) か。 -/
  needsUserGesture : Bool
  /-- その解除を、既に見えている UI への最初の自然な操作 (鍵盤の最初のクリック) に付けたか。
  専用の塞ぐボタンには付けない。 -/
  unlockOnFirstNaturalInteraction : Bool

/-- 条項が認める開き方か。入口のゲートだけを認めない。 -/
def Opening.allowed : Opening -> Prop
  | .primaryUi => True
  | .startGate => False
  | .legalOrSafetyConfirmation => True
  | .destructiveGuard => True
  | .credentialEntry => True

/-- 適用のテスト。対話的な成果物。 -/
def Applies (d : Deliverable) : Prop :=
  d.interactive = true

/-- 主な UI の前に入口のゲートを置かない。 -/
def NoStartGate (d : Deliverable) : Prop :=
  d.opening.allowed

/-- 操作が要るなら、見えている UI への最初の自然な操作で解除する。 -/
def UnlockOnNaturalInteraction (d : Deliverable) : Prop :=
  d.needsUserGesture = true -> d.unlockOnFirstNaturalInteraction = true

/-- 要求。条項が当たる成果物は、開いた瞬間から使える。 -/
def Complies (d : Deliverable) : Prop :=
  Applies d -> NoStartGate d /\ UnlockOnNaturalInteraction d

/-- 鍵盤を隠す「Start Piano」のゲートは、要求を満たさない。 -/
theorem start_gate_fails (gesture unlock : Bool) :
    Not (Complies { interactive := true, opening := Opening.startGate,
                    needsUserGesture := gesture,
                    unlockOnFirstNaturalInteraction := unlock }) := by
  intro h
  exact (h rfl).1

/-- 操作が要るのに専用の塞ぐボタンで解除する成果物は、要求を満たさない。 -/
theorem blocking_unlock_button_fails :
    Not (Complies { interactive := true, opening := Opening.primaryUi,
                    needsUserGesture := true,
                    unlockOnFirstNaturalInteraction := false }) := by
  intro h
  cases (h rfl).2 rfl

/-- 要求を満たす例。鍵盤が最初から見え、最初のクリックで AudioContext を再開する piano。 -/
theorem piano_with_natural_unlock_complies :
    Complies { interactive := true, opening := Opening.primaryUi,
               needsUserGesture := true, unlockOnFirstNaturalInteraction := true } := by
  intro _
  exact And.intro trivial (fun _ => rfl)

/-- 要求を満たす例。credential の入力から始まる成果物は、列挙された例外に当たる。 -/
theorem credential_entry_complies :
    Complies { interactive := true, opening := Opening.credentialEntry,
               needsUserGesture := false, unlockOnFirstNaturalInteraction := false } := by
  intro _
  exact And.intro trivial (fun e => by cases e)

end ImmediateUsabilityNoStartupGates
