/-
  should/0119-reference-by-stable-identifier の Lean 版。uniqnode の条項には UUID の anchor が無い。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  参照の指し方を型で表し、適用のテストと要求を Prop にする。lamalium の同名の条項と違い、
  この版には外で凍結された番号の例外も、同じメッセージの中の対象の除外も無い。
-/
namespace ReferenceByStableIdentifier

/-- 参照が対象を指す方法。 -/
inductive Locator where
  /-- 名前、題、専用の ID。編集で動かない。 -/
  | stableId (name : String)
  /-- 位置 (項番号、セクション番号、行番号)。内容の編集で黙って動く。 -/
  | position (number : Nat)
  /-- 書き手の立ち位置でしか解決できない言い方 (「別のスレッド」「上の項目」「前の依頼」)。 -/
  | description (text : String)

/-- 1 つの参照と、それが置かれた場所。 -/
structure Reference where
  /-- 対象を指す方法。 -/
  locator : Locator
  /-- 操作者や別のセッションへ送るメッセージの中の参照か。 -/
  inMessage : Bool
  /-- その媒体がリンクを描くか。 -/
  mediumRendersLinks : Bool
  /-- 参照がリンクを持つか。 -/
  hasLink : Bool

/-- 読み手が、どこから来ても 1 つの対象に解決できる指し方か。 -/
def Locator.resolvable : Locator -> Prop
  | Locator.stableId _ => True
  | Locator.position _ => False
  | Locator.description _ => False

/-- 適用のテスト。文書、コメント、データの相互参照と、メッセージの中の参照の全て。 -/
def Applies (_r : Reference) : Prop := True

/-- メッセージの中で、媒体がリンクを描くなら、参照はリンクを持つ。 -/
def LinkRequirement (r : Reference) : Prop :=
  r.inMessage = true -> r.mediumRendersLinks = true -> r.hasLink = true

/-- 要求。参照は解決できる指し方で書き、メッセージではリンクを付ける。 -/
def Complies (r : Reference) : Prop :=
  Applies r -> r.locator.resolvable /\ LinkRequirement r

/-- 項目の並び。順序そのものが意味 (優先度など) を持つことがある。 -/
structure OrderedList where
  /-- 順序が意味を持つか。 -/
  orderEncodesMeaning : Bool
  /-- 項目に番号を振っているか。 -/
  hasIndexNumbers : Bool

/-- 適用のテスト。順序が意味を持つ並び。 -/
def OrderedListApplies (l : OrderedList) : Prop := l.orderEncodesMeaning = true

/-- 順序が既に表している意味の上に、番号を重ねない。 -/
def OrderedListComplies (l : OrderedList) : Prop :=
  OrderedListApplies l -> l.hasIndexNumbers = false

/-- メッセージの中で、識別子を得る前の対象に触れるかどうか。 -/
structure Mention where
  /-- 操作者や別のセッションへ送るメッセージの中か。 -/
  inMessage : Bool
  /-- 書き手が対象の識別子を持っているか。 -/
  identifierKnown : Bool
  /-- メッセージが対象に触れているか。 -/
  mentioned : Bool

/-- 適用のテスト。操作者や別のセッションへ送るメッセージの中の言及。 -/
def MentionApplies (m : Mention) : Prop := m.inMessage = true

/-- 識別子を得るまで、メッセージは対象に触れない。 -/
def MentionComplies (m : Mention) : Prop :=
  MentionApplies m -> m.mentioned = true -> m.identifierKnown = true

/-- 「別のスレッド」のような言い方は、リンクの有無にかかわらず要求を満たさない。 -/
theorem description_never_complies (text : String) (inMessage links hasLink : Bool) :
    Not (Complies { locator := Locator.description text, inMessage := inMessage,
                    mediumRendersLinks := links, hasLink := hasLink }) := by
  intro h
  exact (h trivial).1

/-- 項番号や行番号による参照は要求を満たさない。 -/
theorem position_never_complies (number : Nat) (inMessage links hasLink : Bool) :
    Not (Complies { locator := Locator.position number, inMessage := inMessage,
                    mediumRendersLinks := links, hasLink := hasLink }) := by
  intro h
  exact (h trivial).1

/-- 順序が優先度を表す並びに番号を振ると、要求を満たさない。 -/
theorem index_numbers_on_ordered_list_fail :
    Not (OrderedListComplies { orderEncodesMeaning := true, hasIndexNumbers := true }) := by
  intro h
  have e : true = false := h rfl
  cases e

/-- 要求を満たす例。別のスレッドを題名とリンクで指す返信。 -/
theorem titled_thread_link_complies :
    Complies { locator := Locator.stableId "Fix Safari export button", inMessage := true,
               mediumRendersLinks := true, hasLink := true } := by
  intro _
  exact And.intro trivial (fun _ _ => rfl)

/-- 識別子が無いうちに対象に触れたメッセージは、要求を満たさない。 -/
theorem mention_without_identifier_fails :
    Not (MentionComplies { inMessage := true, identifierKnown := false, mentioned := true }) := by
  intro h
  have e : false = true := h rfl rfl
  cases e

end ReferenceByStableIdentifier
