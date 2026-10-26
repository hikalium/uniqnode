/-
  must/0013-docs-design-plan-split の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  docs/ の文書を design と plan の 2 つに分ける置き方と、文書への参照の形 (UUID アンカー) を Prop にする。
-/
namespace DocsDesignPlanSplit

/-- docs/ の中の 2 つの半分。 -/
inductive Half where
  /-- docs/design/。今の実装を、時制のない現在の記述として書く。 -/
  | design
  /-- docs/plan/。これからの仕事を、完了条件つきの作業項目として書く。 -/
  | plan

/-- docs/ の下の 1 つの文書。 -/
structure Document where
  /-- 文書が置かれた半分。 -/
  half : Half
  /-- docs/ の下にあるか。 -/
  underDocs : Bool
  /-- 未実装の部分を含むか。 -/
  hasUnimplementedPart : Bool
  /-- 変更履歴、進捗ログ、実装完了日付の物語、試行錯誤の経過を含むか。 -/
  hasChronicle : Bool
  /-- 着地した plan の項目を残しているか。 -/
  keepsLandedItem : Bool
  /-- plan の文書として、仕事が全て着地したのにファイルが残っているか。 -/
  keptAfterWorkFullyLanded : Bool
  /-- 分類を見出しではなくコードと突き合わせて検証したか。 -/
  classificationVerifiedAgainstCode : Bool
  /-- H1 の直下に `<a id="<uuid>"></a>` のアンカーを持つか。 -/
  hasAnchorUnderH1 : Bool
  /-- 木について既に正しくなくなった情報 (古くなった設計の事実や plan の記述) を残しているか。 -/
  hasStaleFact : Bool

/-- 適用のテスト。docs/ の下の文書。 -/
def Applies (d : Document) : Prop :=
  d.underDocs = true

/-- design/ の文書は、未実装の部分も年代記も持たない。 -/
def DesignHalfIsCurrent (d : Document) : Prop :=
  d.half = Half.design -> d.hasUnimplementedPart = false /\ d.hasChronicle = false

/-- plan/ の文書は、着地した項目を残さず、全て着地したら削除される。 -/
def PlanHalfIsFuture (d : Document) : Prop :=
  d.half = Half.plan -> d.keepsLandedItem = false /\ d.keptAfterWorkFullyLanded = false

/-- 分類はコードと突き合わせて検証し、H1 の直下にアンカーを持つ。 -/
def ClassifiedAndAnchored (d : Document) : Prop :=
  d.classificationVerifiedAgainstCode = true /\ d.hasAnchorUnderH1 = true

/-- どちらの側でも、古くなった情報は削除され、最新の情報だけが残る。 -/
def OutdatedDeleted (d : Document) : Prop :=
  d.hasStaleFact = false

/-- 要求。 -/
def Complies (d : Document) : Prop :=
  Applies d ->
    DesignHalfIsCurrent d /\ PlanHalfIsFuture d /\ ClassifiedAndAnchored d /\ OutdatedDeleted d

/-- 参照が書かれる場所。 -/
inductive Site where
  /-- 散文。 -/
  | prose
  /-- コードのコメントか文字列。 -/
  | codeOrString

/-- 参照の形。 -/
inductive RefForm where
  /-- `[docs/design|plan/NAME.md](#<uuid>)` の読める形。 -/
  | readableLink
  /-- `NAME (uuid:<uuid>)` のパスを含まない形。 -/
  | pathFree
  /-- docs/ へのパスによるリンク。 -/
  | pathLink

/-- docs/ の文書への 1 つの参照。 -/
structure Reference where
  /-- 参照が書かれた場所。 -/
  site : Site
  /-- 参照の形。 -/
  form : RefForm
  /-- 参照先が docs/ の下の文書か。 -/
  targetUnderDocs : Bool
  /-- セクションを指すとき、そのセクションの見出しの下にアンカーがあり、リンクがそれを指すか。文書全体を指すときは true。 -/
  sectionAnchorDirect : Bool

/-- 適用のテスト。docs/ の文書への参照。 -/
def ReferenceApplies (r : Reference) : Prop :=
  r.targetUnderDocs = true

/-- 場所に合った形。散文は読める形、コードはパスを含まない形。 -/
def FormFitsSite (r : Reference) : Prop :=
  (r.site = Site.prose -> r.form = RefForm.readableLink) /\
  (r.site = Site.codeOrString -> r.form = RefForm.pathFree)

/-- 参照への要求。 -/
def ReferenceComplies (r : Reference) : Prop :=
  ReferenceApplies r -> FormFitsSite r /\ r.sectionAnchorDirect = true

/-- 仕事が全て着地したのに「履歴記録」として残した plan の文書は、要求を満たさない。 -/
theorem finished_plan_kept_as_history_fails :
    Not (Complies { half := Half.plan, underDocs := true, hasUnimplementedPart := false,
                    hasChronicle := false, keepsLandedItem := true,
                    keptAfterWorkFullyLanded := true, classificationVerifiedAgainstCode := true,
                    hasAnchorUnderH1 := true, hasStaleFact := true }) := by
  intro h
  have e : true = false := ((h rfl).2.1 rfl).2
  cases e

/-- 既に正しくない事実を残した design/ の文書は、要求を満たさない。 -/
theorem stale_design_fact_fails :
    Not (Complies { half := Half.design, underDocs := true, hasUnimplementedPart := false,
                    hasChronicle := false, keepsLandedItem := false,
                    keptAfterWorkFullyLanded := false, classificationVerifiedAgainstCode := true,
                    hasAnchorUnderH1 := true, hasStaleFact := true }) := by
  intro h
  have e : true = false := (h rfl).2.2.2
  cases e

/-- コードのコメントの中のパスによるリンクは、要求を満たさない。 -/
theorem path_link_in_code_fails :
    Not (ReferenceComplies { site := Site.codeOrString, form := RefForm.pathLink,
                             targetUnderDocs := true, sectionAnchorDirect := true }) := by
  intro h
  have e : RefForm.pathLink = RefForm.pathFree := (h rfl).1.2 rfl
  cases e

/-- 要求を満たす例。今の実装だけを書き、アンカーを持つ design/ の文書。 -/
theorem current_design_document_complies :
    Complies { half := Half.design, underDocs := true, hasUnimplementedPart := false,
               hasChronicle := false, keepsLandedItem := false,
               keptAfterWorkFullyLanded := false, classificationVerifiedAgainstCode := true,
               hasAnchorUnderH1 := true, hasStaleFact := false } := by
  intro _
  exact And.intro (fun _ => And.intro rfl rfl)
    (And.intro (fun _ => And.intro rfl rfl) (And.intro (And.intro rfl rfl) rfl))

/-- 要求を満たす例。コードのコメントの中の `NAME (uuid:<uuid>)` の形の参照。 -/
theorem path_free_code_reference_complies :
    ReferenceComplies { site := Site.codeOrString, form := RefForm.pathFree,
                        targetUnderDocs := true, sectionAnchorDirect := true } := by
  intro _
  exact And.intro (And.intro (fun e => by cases e) (fun _ => rfl)) rfl

end DocsDesignPlanSplit
