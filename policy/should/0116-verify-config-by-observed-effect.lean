/-
  should/0116-verify-config-by-observed-effect の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  設定の変更を構造にし、効果の観察の仕方を適用の前に決め、適用の後に稼働系の実効状態で検証する
  ことを要求として表す。編集したファイルや終了状態は効果ではない。
-/
namespace VerifyConfigByObservedEffect

/-- 変更が効いたとする根拠。 -/
inductive Evidence where
  /-- 稼働系の実効状態 (稼働中の設定の出力、資源の使用、測った振る舞い)。 -/
  | effectiveState
  /-- 編集したファイルの中身。 -/
  | editedFile
  /-- 適用の段階の終了状態。 -/
  | applyExitCode
  /-- 「起動した」という知らせ。 -/
  | startedMessage
  /-- 根拠なし。 -/
  | none

/-- 意図した効果そのものを見た根拠か。 -/
def Evidence.isObservedEffect : Evidence -> Prop
  | .effectiveState => True
  | .editedFile => False
  | .applyExitCode => False
  | .startedMessage => False
  | .none => False

/-- 設定 (サービスの定義、環境変数、実行時やカーネルの設定、設定ファイル) への 1 つの変更。 -/
structure Change where
  /-- 設定に触る変更か。 -/
  touchesConfiguration : Bool
  /-- 適用の前に、効果をどう観察するかを決めたか。 -/
  observationDecidedBeforeApply : Bool
  /-- 適用の後に効いたとした根拠。 -/
  evidence : Evidence
  /-- 完了と報告したか。 -/
  reportedDone : Bool

/-- 適用のテスト。設定に触る変更。 -/
def Applies (c : Change) : Prop :=
  c.touchesConfiguration = true

/-- 要求。観察の仕方を先に決め、完了と報告するなら意図した効果を稼働系で観察してある。 -/
def Complies (c : Change) : Prop :=
  Applies c ->
    c.observationDecidedBeforeApply = true /\ (c.reportedDone = true -> c.evidence.isObservedEffect)

/-- 条項が止めたい形 (2026-07)。unit ファイルを編集し、適用の終了状態が 0 だったので完了と報告する。 -/
theorem exit_code_as_effect_fails (decided : Bool) :
    Not (Complies { touchesConfiguration := true, observationDecidedBeforeApply := decided,
                    evidence := Evidence.applyExitCode, reportedDone := true }) := by
  intro h
  exact (h rfl).2 rfl

/-- 「起動した」も効果ではない。1/8 の速さの代替の経路で動くサービスも起動はする。 -/
theorem started_message_fails :
    Not (Complies { touchesConfiguration := true, observationDecidedBeforeApply := true,
                    evidence := Evidence.startedMessage, reportedDone := true }) := by
  intro h
  exact (h rfl).2 rfl

/-- 要求を満たす例。観察の仕方を決めてから適用し、稼働中の設定の出力で効果を見てから報告する。 -/
theorem observed_effect_complies :
    Complies { touchesConfiguration := true, observationDecidedBeforeApply := true,
               evidence := Evidence.effectiveState, reportedDone := true } := by
  intro _
  exact And.intro rfl (fun _ => trivial)

end VerifyConfigByObservedEffect
