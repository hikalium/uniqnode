/-
  must/0019-no-silent-discard-of-model-output の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  モデルの出力を機械で読む 1 つの場所を主題にし、形が合わないときに開いて失敗するか見える形で拒否するかの
  どちらかを取り、静かに空にしないこと、合わない入力のテストがあることを Prop にする。
-/
namespace NoSilentDiscardOfModelOutput

/-- 形が合わなかったときに、その場所が取る振る舞い。 -/
inductive OnMismatch where
  /-- 開いて失敗する。生の文をそのまま下流へ渡す (要れば未解析とマークを付ける)。 -/
  | failOpen
  /-- 見える形で拒否する。期待した形を名指す拒否をモデルへ返し、ログの事象を出す。 -/
  | rejectVisibly
  /-- 静かに空の結果を返す。内容が消える。 -/
  | quietEmpty

/-- モデルの出力を機械で読む 1 つの場所 (ツール呼び出しの解析、本文とフェンスの扱い、判定の読み手)。 -/
structure ParserSite where
  /-- モデルの出力を機械で読む場所か。読むとは、期待した形と突き合わせ、合わないと判定することである。 -/
  machineReadsModelOutput : Bool
  /-- 形が合わなかったときの振る舞い。 -/
  onMismatch : OnMismatch
  /-- 合わない入力を与え、内容が残るか拒否が明示かを断定するテストがあるか。 -/
  nonconformingInputTested : Bool

/-- 適用のテスト。モデルの出力を機械で読む全ての場所。 -/
def Applies (s : ParserSite) : Prop :=
  s.machineReadsModelOutput = true

/-- 内容は消えない。開いて失敗するか、見える形で拒否する。 -/
def NothingVanishes (s : ParserSite) : Prop :=
  s.onMismatch = OnMismatch.failOpen \/ s.onMismatch = OnMismatch.rejectVisibly

/-- 合わない入力のテストがある。 -/
def Tested (s : ParserSite) : Prop :=
  s.nonconformingInputTested = true

/-- 要求。 -/
def Complies (s : ParserSite) : Prop :=
  Applies s -> NothingVanishes s /\ Tested s

/-- 形が合わないと静かに空になる場所は、どれほど起きにくく見えても要求を満たさない。 -/
theorem quiet_empty_fails (t : Bool) :
    Not (Complies { machineReadsModelOutput := true, onMismatch := OnMismatch.quietEmpty,
                    nonconformingInputTested := t }) := by
  intro h
  have hr := h rfl
  have v : OnMismatch.quietEmpty = OnMismatch.failOpen \/
      OnMismatch.quietEmpty = OnMismatch.rejectVisibly := hr.1
  cases v with
  | inl e => cases e
  | inr e => cases e

/-- 開いて失敗しても、合わない入力のテストが無い場所は要求を満たさない。 -/
theorem untested_site_fails :
    Not (Complies { machineReadsModelOutput := true, onMismatch := OnMismatch.failOpen,
                    nonconformingInputTested := false }) := by
  intro h
  have hr := h rfl
  have t : false = true := hr.2
  cases t

/-- 要求を満たす例。フェンスの契約に合わない本文を、未解析のマークを付けてそのまま通す書き込みの経路。 -/
theorem fail_open_write_complies :
    Complies { machineReadsModelOutput := true, onMismatch := OnMismatch.failOpen,
               nonconformingInputTested := true } := by
  intro _
  exact And.intro (Or.inl rfl) rfl

/-- 要求を満たす例。期待した形を名指す拒否をモデルへ返すツール呼び出しの解析。 -/
theorem reject_visibly_complies :
    Complies { machineReadsModelOutput := true, onMismatch := OnMismatch.rejectVisibly,
               nonconformingInputTested := true } := by
  intro _
  exact And.intro (Or.inr rfl) rfl

end NoSilentDiscardOfModelOutput
