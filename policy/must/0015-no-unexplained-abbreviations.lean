/-
  must/0015-no-unexplained-abbreviations の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  読み手が解読しなければならない 1 つの名前を主題にし、略語を作るなら理由をその場に記すことを Prop にする。
  既存の名前は、周りのコードを組み直すときに条項の対象に入る。
-/
namespace NoUnexplainedAbbreviations

/-- 名前の形。 -/
inductive Form where
  /-- 完全な語 (`supervisor`、`request`、`configuration`、標準の `config`)。 -/
  | fullWord
  /-- 読み手が既に知っている標準の語 (`id`、`max`、`min`、`len`、`URL`、`HTTP`、`LLM`、`TOML`)。 -/
  | standardTerm
  /-- 範囲の狭い慣用の局所名 (ループの添字 `i`、短いクロージャの引数)。 -/
  | tightScopeLocal
  /-- 作られた略語。切り詰め、母音落とし、内輪の頭字語 (`sup`、`req`、`cfg`)。 -/
  | coinedAbbreviation

/-- 読み手が解読する 1 つの名前 (変数、関数、型、モジュール、ファイル名、サブコマンド、設定のキー)。 -/
structure Name where
  /-- 読み手が、名前の指す概念に届くために名前を解読しなければならないか。 -/
  readerMustDecode : Bool
  /-- 名前の形。 -/
  form : Form
  /-- 明確な理由 (外から課された長さの上限など) が、名前を導入した場所に記されているか。 -/
  reasonRecorded : Bool
  /-- この条項より前からある名前か。 -/
  preexisting : Bool
  /-- 周りのコードが今まさに組み直されている (別のモジュールへ移すなど) か。 -/
  surroundingCodeRestructured : Bool

/-- 適用のテスト。読み手が解読する名前のうち、新しいものか、周りのコードが組み直されているもの。
    組み直されていない既存の名前は、そのまま残してよい。 -/
def Applies (n : Name) : Prop :=
  n.readerMustDecode = true /\
    (n.preexisting = false \/ n.surroundingCodeRestructured = true)

/-- 略語を作るなら、明確な理由をその場に記す。 -/
def AbbreviationHasReason (n : Name) : Prop :=
  n.form = Form.coinedAbbreviation -> n.reasonRecorded = true

/-- 要求。 -/
def Complies (n : Name) : Prop :=
  Applies n -> AbbreviationHasReason n

/-- 記録された、効力のある例。main.rs の分解で退けられた、理由の無い `sup` は要求を満たさない。 -/
theorem sup_without_reason_fails :
    Not (Complies { readerMustDecode := true, form := Form.coinedAbbreviation,
                    reasonRecorded := false, preexisting := false,
                    surroundingCodeRestructured := true }) := by
  intro h
  cases h (And.intro rfl (Or.inl rfl)) rfl

/-- 組み直されるモジュールに残った既存の略語も、理由が無ければ要求を満たさない。 -/
theorem restructured_old_abbreviation_fails :
    Not (Complies { readerMustDecode := true, form := Form.coinedAbbreviation,
                    reasonRecorded := false, preexisting := true,
                    surroundingCodeRestructured := true }) := by
  intro h
  cases h (And.intro rfl (Or.inr rfl)) rfl

/-- 要求を満たす例。完全な語 `supervisor`。 -/
theorem supervisor_complies :
    Complies { readerMustDecode := true, form := Form.fullWord, reasonRecorded := false,
               preexisting := false, surroundingCodeRestructured := false } := by
  intro _
  intro e
  cases e

/-- 要求を満たす例。外から課された長さの上限を導入の場所に記した略語。 -/
theorem reasoned_abbreviation_complies :
    Complies { readerMustDecode := true, form := Form.coinedAbbreviation, reasonRecorded := true,
               preexisting := false, surroundingCodeRestructured := false } := by
  intro _
  intro _
  rfl

/-- 組み直されていない既存の名前は対象外なので、何も求められない。 -/
theorem untouched_preexisting_name_complies (f : Form) :
    Complies { readerMustDecode := true, form := f, reasonRecorded := false,
               preexisting := true, surroundingCodeRestructured := false } := by
  intro h
  have d := And.right h
  cases d with
  | inl e => cases e
  | inr e => cases e

end NoUnexplainedAbbreviations
