/-
  must/0008-no-python-rust-only の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。このプロジェクトの実行可能なコードの
  1 つの単位を型で表し、適用のテストと要求 (Python を使わず、実装は Rust) を Prop にする。
  Rust の依存の取得の経路 (cargo vendor と allowlist の proxy) は要求の外の注として扱わない。
-/
namespace NoPythonRustOnly

/-- コードの単位が書かれている言語。 -/
inductive Language where
  /-- Rust (lamalium のバイナリ、ホストの道具、エージェントの成果物)。 -/
  | rust
  /-- Python (python、python3、python -c、.py、-m http.server)。 -/
  | python
  /-- コンテナの POSIX の道具 (sh、awk、curl) を糊として使うエージェントの run コマンド。 -/
  | posixGlue
  /-- 宣言的な設定と文書 (*.toml、*.network、*.service、Markdown)。実装ではない。 -/
  | declarative

/-- このプロジェクトのコードの 1 つの単位。コンテナの中も、ホストの道具も、エージェントが作るか走らせるものも含む。 -/
structure Code where
  /-- 書かれている言語。 -/
  language : Language
  /-- Python のインタプリタを呼ぶか (Rust の Command からでも、糊のシェルからでも)。 -/
  invokesPython : Bool
  /-- 自明でないプログラムか (糊を超える論理)。 -/
  nonTrivialProgram : Bool

/-- 適用のテスト。実行可能なコード。宣言的な設定と文書は実装ではないので外。 -/
def Applies (c : Code) : Prop :=
  Not (c.language = Language.declarative)

/-- Python で書かない。 -/
def NotWrittenInPython (c : Code) : Prop :=
  Not (c.language = Language.python)

/-- Python のインタプリタを呼ばない。イメージに python3 があることは、呼ぶ許可ではない。 -/
def NoInterpreterCall (c : Code) : Prop :=
  c.invokesPython = false

/-- 自明でないプログラムは Rust で書く。糊のシェルは自明なものに限る。 -/
def NonTrivialIsRust (c : Code) : Prop :=
  c.nonTrivialProgram = true -> c.language = Language.rust

/-- 要求。 -/
def Complies (c : Code) : Prop :=
  Applies c -> NotWrittenInPython c /\ NoInterpreterCall c /\ NonTrivialIsRust c

/-- python -c で書いた診断は要求を満たさない。 -/
theorem python_snippet_fails :
    Not (Complies { language := Language.python, invokesPython := true,
                    nonTrivialProgram := false }) := by
  intro h
  have applies : Applies { language := Language.python, invokesPython := true,
                           nonTrivialProgram := false } := by
    intro e
    cases e
  exact (h applies).1 rfl

/-- 糊のシェルから -m http.server を呼ぶ run コマンドも、要求を満たさない。 -/
theorem glue_calling_python_fails :
    Not (Complies { language := Language.posixGlue, invokesPython := true,
                    nonTrivialProgram := false }) := by
  intro h
  have applies : Applies { language := Language.posixGlue, invokesPython := true,
                           nonTrivialProgram := false } := by
    intro e
    cases e
  cases (h applies).2.1

/-- 自明でないプログラムを糊のシェルで組むのも、要求を満たさない。 -/
theorem non_trivial_glue_fails :
    Not (Complies { language := Language.posixGlue, invokesPython := false,
                    nonTrivialProgram := true }) := by
  intro h
  have applies : Applies { language := Language.posixGlue, invokesPython := false,
                           nonTrivialProgram := true } := by
    intro e
    cases e
  cases (h applies).2.2 rfl

/-- 要求を満たす例。std::net::TcpListener で書いた Rust の web サーバー。 -/
theorem rust_web_server_complies :
    Complies { language := Language.rust, invokesPython := false, nonTrivialProgram := true } := by
  intro _
  refine And.intro ?_ (And.intro rfl (fun _ => rfl))
  intro e
  cases e

/-- 要求を満たす例。sh と awk だけを糊に使う自明な run コマンド。 -/
theorem posix_glue_complies :
    Complies { language := Language.posixGlue, invokesPython := false,
               nonTrivialProgram := false } := by
  intro _
  refine And.intro ?_ (And.intro rfl ?_)
  intro e
  cases e
  intro e
  cases e

end NoPythonRustOnly
