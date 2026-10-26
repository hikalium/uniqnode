/-
  must/0018-no-unnamed-tuples-of-four-or-more の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。このリポジトリの Rust のソース
  (container/src/、container/tests/、container/build.rs) にある 1 つのタプルの構文を型で表し、
  適用のテストと要求を Prop にする。
-/
namespace NoUnnamedTuplesOfFourOrMore

/-- タプルの構文の種類。 -/
inductive Kind where
  /-- タプル型 (Vec<(A, B, C, D)>)。 -/
  | tupleType
  /-- タプルの式か分解のパターン。 -/
  | tupleExpression
  /-- タプル構造体の宣言 (struct Name(A, B, C, D);)。 -/
  | tupleStruct
  /-- enum のタプルの variant (enum E { V(A, B, C, D) })。 -/
  | enumTupleVariant

/-- リポジトリの Rust のソースにある 1 つのタプルの構文。 -/
structure TupleConstruct where
  /-- 構文の種類。 -/
  kind : Kind
  /-- 要素の数。 -/
  arity : Nat
  /-- 要素のどれかに名前があるか。 -/
  anyMemberNamed : Bool

/-- 適用のテスト。このリポジトリの Rust のソースにある全てのタプルの構文。 -/
def Applies (_t : TupleConstruct) : Prop := True

/-- 要求。名前の無い要素が 4 つ以上のタプルを使わない。既存の違反も変換されたので、例外は無い。 -/
def Complies (t : TupleConstruct) : Prop :=
  Applies t -> t.anyMemberNamed = true \/ t.arity <= 3

/-- 2026-07-24 の feed の 6 つ組 (i64, String, &'static str, bool, String, String) は要求を満たさない。 -/
theorem six_tuple_fails :
    Not (Complies { kind := Kind.tupleType, arity := 6, anyMemberNamed := false }) := by
  intro h
  have p : false = true \/ 6 <= 3 := h trivial
  cases p with
  | inl e => cases e
  | inr e => exact absurd e (by decide : Not (6 <= 3))

/-- struct Name(A, B, C, D); も要求を満たさない。 -/
theorem four_tuple_struct_fails :
    Not (Complies { kind := Kind.tupleStruct, arity := 4, anyMemberNamed := false }) := by
  intro h
  have p : false = true \/ 4 <= 3 := h trivial
  cases p with
  | inl e => cases e
  | inr e => exact absurd e (by decide : Not (4 <= 3))

/-- 要求を満たす例。3 つ組は読める。 -/
theorem triple_complies :
    Complies { kind := Kind.tupleExpression, arity := 3, anyMemberNamed := false } := by
  intro _
  exact Or.inr (by decide : 3 <= 3)

end NoUnnamedTuplesOfFourOrMore
