/-
  should/0128-check-the-channel-before-claiming-absence の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  稼働のログや計器から導いた「無かった」という主張を主題にして、適用のテストと要求を Prop にする。
-/
namespace CheckTheChannelBeforeClaimingAbsence

/-- 観測の経路から導いた 1 つの主張。 -/
structure Claim where
  /-- 「X は一度も起きなかった」「0 件」のような不在の主張か。 -/
  isAbsenceClaim : Bool
  /-- 稼働のログや計器から導いた主張か。 -/
  derivedFromChannel : Bool
  /-- 数えた経路が X を実際に観測できると検証したか。 -/
  channelObservabilityConfirmed : Bool
  /-- 表示の切り詰め、絞り込み、瞬間の標本といった死角が X を飲み込みうるか。 -/
  blindSpotCouldSwallow : Bool
  /-- 生のペイロード (状態ファイル、応答の本体、成果物) で検証したか。 -/
  verifiedAgainstRawPayload : Bool
  /-- どの経路に基づく主張かを書いたか。 -/
  channelNamed : Bool
  /-- 探索の覚え書きで、未検証の仮の 0 とマークを付けたか。 -/
  markedUnverified : Bool

/-- 適用のテスト。経路から導いた不在の主張。肯定の主張 (「X は N 回起きた」) は対象外。 -/
def Applies (c : Claim) : Prop :=
  c.isAbsenceClaim = true /\ c.derivedFromChannel = true

/-- 経路が X を観測できると検証し、死角が飲み込みうるなら生のペイロードで検証する。 -/
def ChannelChecked (c : Claim) : Prop :=
  c.channelObservabilityConfirmed = true /\
  (c.blindSpotCouldSwallow = true -> c.verifiedAgainstRawPayload = true)

/-- どの経路に基づく主張かを書く。 -/
def ChannelNamed (c : Claim) : Prop :=
  c.channelNamed = true

/-- 要求。事実として述べるなら経路を検証して名指す。探索の覚え書きの仮の 0 は、未検証とマークを付ければよい。 -/
def Complies (c : Claim) : Prop :=
  Applies c -> (c.markedUnverified = true \/ (ChannelChecked c /\ ChannelNamed c))

/-- 切り詰められた ctl log の表示の grep から、死角を見ずに事実として述べた 0 は、要求を満たさない。 -/
theorem zero_from_truncated_view_fails :
    Not (Complies { isAbsenceClaim := true, derivedFromChannel := true,
                    channelObservabilityConfirmed := false, blindSpotCouldSwallow := true,
                    verifiedAgainstRawPayload := false, channelNamed := true,
                    markedUnverified := false }) := by
  intro h
  cases h (And.intro rfl rfl) with
  | inl e => cases e
  | inr k =>
    have e : false = true := k.1.1
    cases e

/-- 肯定の主張は適用の対象外である。 -/
theorem positive_claim_outside (c : Claim) (pos : c.isAbsenceClaim = false) :
    Not (Applies c) := by
  intro a
  have e : false = true := pos.symm.trans a.1
  cases e

/-- 要求を満たす例。死角を生のペイロードで検証し、経路を名指した 0。 -/
theorem raw_payload_verified_zero_complies :
    Complies { isAbsenceClaim := true, derivedFromChannel := true,
               channelObservabilityConfirmed := true, blindSpotCouldSwallow := true,
               verifiedAgainstRawPayload := true, channelNamed := true,
               markedUnverified := false } := by
  intro _
  exact Or.inr (And.intro (And.intro rfl (fun _ => rfl)) rfl)

/-- 要求を満たす例。探索の覚え書きの、未検証とマークを付けた仮の 0。 -/
theorem marked_tentative_zero_complies :
    Complies { isAbsenceClaim := true, derivedFromChannel := true,
               channelObservabilityConfirmed := false, blindSpotCouldSwallow := true,
               verifiedAgainstRawPayload := false, channelNamed := false,
               markedUnverified := true } := by
  intro _
  exact Or.inl rfl

end CheckTheChannelBeforeClaimingAbsence
