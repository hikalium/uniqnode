/-
  must/0010-delete-superseded-docs の Lean 版。uniqnode の条項には UUID の anchor が無い。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  木の中の 1 つの説明、1 つのコードのコメント、退役した条項の stub を 3 つの主題にし、
  真でなくなった説明は消すこと、コメントは現在だけを述べること、stub の形を Prop にする。
-/
namespace DeleteSupersededDocs

/-- 木の中で設計や振る舞いを述べる 1 つの説明 (DESIGN.md、docs/、policy/、plan、説明のコメント)。 -/
structure Description where
  /-- 作業の木の中の散文か。git の履歴は外。 -/
  inWorkingTree : Bool
  /-- いまも真か。 -/
  stillTrue : Bool
  /-- まだ木に残っているか。 -/
  present : Bool
  /-- 「superseded」「historical」「deprecated」とマークを付けて残したか。 -/
  markedSuperseded : Bool
  /-- コメントアウトして残したか。 -/
  commentedOut : Bool

/-- 適用のテスト。作業の木の中の説明。 -/
def Applies (d : Description) : Prop := d.inWorkingTree = true

/-- 要求。真でなくなった説明は木から消し、マークを付けても、コメントアウトしても残さない。 -/
def Complies (d : Description) : Prop :=
  Applies d -> d.stillTrue = false -> d.present = false /\ d.markedSuperseded = false /\
  d.commentedOut = false

/-- コードの 1 つのコメント。 -/
structure Comment where
  /-- 追跡されるコードの中のコメントか。 -/
  inWorkingTree : Bool
  /-- 将来の仕事の TODO のマークか。 -/
  isTodo : Bool
  /-- 出自や変更の履歴 (旧 X の置換、was previously、moved from) を語るか。 -/
  narratesHistory : Bool

/-- 適用のテスト。コードのコメント。 -/
def CommentApplies (c : Comment) : Prop := c.inWorkingTree = true

/-- TODO 以外のコメントは現在だけを述べ、差分を語らない。 -/
def CommentComplies (c : Comment) : Prop :=
  CommentApplies c -> c.isTodo = false -> c.narratesHistory = false

/-- 退役した条項に残す stub。木の中で、置き換えられたものを消さずに残す唯一の場所。 -/
structure RetiredStub where
  /-- 退役した条項か。 -/
  retired : Bool
  /-- ファイル名が `policy/<level>/NNNN-obsoleted.md` か。 -/
  renamedObsoleted : Bool
  /-- 1 行目が「This policy is obsoleted. Remove reference to this policy.」か。 -/
  firstLineIsRedirect : Bool
  /-- 古い本文を残しているか。 -/
  oldBodyKept : Bool
  /-- 1 行目の後が、移した先の条項の id だけか (無ければ空)。 -/
  onlySuccessorIds : Bool

/-- 適用のテスト。退役した条項。 -/
def RetiredStubApplies (s : RetiredStub) : Prop := s.retired = true

/-- stub は改名され、1 行目の指示と移し先の id だけを持ち、古い本文を持たない。 -/
def RetiredStubComplies (s : RetiredStub) : Prop :=
  RetiredStubApplies s ->
    s.renamedObsoleted = true /\ s.firstLineIsRedirect = true /\ s.oldBodyKept = false /\
    s.onlySuccessorIds = true

/-- 「deprecated」のマークを付けて残した古い説明は、要求を満たさない。 -/
theorem marked_superseded_fails (commented : Bool) :
    Not (Complies { inWorkingTree := true, stillTrue := false, present := true,
                    markedSuperseded := true, commentedOut := commented }) := by
  intro h
  have e : true = false := (h rfl rfl).1
  cases e

/-- 古い本文を残した退役の stub は、要求を満たさない。 -/
theorem stub_keeping_old_body_fails :
    Not (RetiredStubComplies { retired := true, renamedObsoleted := true,
                               firstLineIsRedirect := true, oldBodyKept := true,
                               onlySuccessorIds := false }) := by
  intro h
  have e : true = false := (h rfl).2.2.1
  cases e

/-- 要求を満たす例。振る舞いが変わったときに、その場で現在の真実に書き換えた説明。 -/
theorem rewritten_in_place_complies :
    Complies { inWorkingTree := true, stillTrue := true, present := true,
               markedSuperseded := false, commentedOut := false } := by
  intro _ e
  cases e

/-- 要求を満たす例。消した古い説明。 -/
theorem deleted_description_complies :
    Complies { inWorkingTree := true, stillTrue := false, present := false,
               markedSuperseded := false, commentedOut := false } := by
  intro _ _
  exact And.intro rfl (And.intro rfl rfl)

/-- 要求を満たす例。現在の振る舞いと、それを正当化する測定の結果だけを述べるコメント。 -/
theorem present_tense_comment_complies :
    CommentComplies { inWorkingTree := true, isTodo := false, narratesHistory := false } := by
  intro _ _
  exact rfl

/-- 要求を満たす例。改名され、指示の行と移し先の id だけを持つ stub。 -/
theorem redirect_stub_complies :
    RetiredStubComplies { retired := true, renamedObsoleted := true, firstLineIsRedirect := true,
                          oldBodyKept := false, onlySuccessorIds := true } := by
  intro _
  exact And.intro rfl (And.intro rfl (And.intro rfl rfl))

end DeleteSupersededDocs
