/-
  should/0112-externalize-embedded-assets の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。Rust のソースに置かれた他言語の
  文字列を型で表し、適用のテスト (大きいか) と要求 (自分のファイルに置き、コンパイル時に取り込む) を Prop にする。
-/
namespace ExternalizeEmbeddedAssets

/-- Rust のコードが持つ、他の言語 (HTML、CSS、JS、SQL、テンプレート、テストのフィクスチャ) の 1 つの文字列。
    エージェントの成果物とハーネスのコードの両方。 -/
structure ForeignString where
  /-- 行数。 -/
  lines : Nat
  /-- それを含む .rs ファイルが、その資産そのものより読みにくく編集しにくくなっているか。 -/
  burdensRustFile : Bool
  /-- ソースの隣の、中身を表す名前のファイル (page.html、style.css) に置いてあるか。 -/
  inOwnFile : Bool
  /-- include_str! でコンパイル時に結び付けているか。 -/
  boundAtCompileTime : Bool

/-- 適用のテスト。大きい文字列。おおよそ 20 行を超えるか、.rs ファイルを資産より読みにくくするもの。 -/
def Applies (s : ForeignString) : Prop :=
  20 < s.lines \/ s.burdensRustFile = true

/-- 自分のファイルに置く。 -/
def InOwnFile (s : ForeignString) : Prop :=
  s.inOwnFile = true

/-- コンパイル時の取り込みで結び付ける (実行時の I/O やパスの扱いを持ち込まない)。 -/
def BoundAtCompileTime (s : ForeignString) : Prop :=
  s.boundAtCompileTime = true

/-- 要求。 -/
def Complies (s : ForeignString) : Prop :=
  Applies s -> InOwnFile s /\ BoundAtCompileTime s

/-- t-site の 17.8KB のページを .rs の中にそのまま書いた成果物は、要求を満たさない。 -/
theorem inline_page_fails :
    Not (Complies { lines := 400, burdensRustFile := true, inOwnFile := false,
                    boundAtCompileTime := false }) := by
  intro h
  cases (h (Or.inr rfl)).1

/-- 自分のファイルにあっても、実行時に読むなら要求を満たさない。 -/
theorem runtime_read_fails :
    Not (Complies { lines := 400, burdensRustFile := false, inOwnFile := true,
                    boundAtCompileTime := false }) := by
  intro h
  cases (h (Or.inl (by decide : 20 < 400))).2

/-- 要求を満たす例。pub const PAGE: &str = include_str!("page.html")。 -/
theorem include_str_page_complies :
    Complies { lines := 400, burdensRustFile := false, inOwnFile := true,
               boundAtCompileTime := true } := by
  intro _
  exact And.intro rfl rfl

/-- 数行の短い文字列には当たらない。 -/
theorem short_string_is_out_of_scope :
    Complies { lines := 3, burdensRustFile := false, inOwnFile := false,
               boundAtCompileTime := false } := by
  intro h
  cases h with
  | inl e => exact absurd e (by decide : Not (20 < 3))
  | inr e => cases e

end ExternalizeEmbeddedAssets
