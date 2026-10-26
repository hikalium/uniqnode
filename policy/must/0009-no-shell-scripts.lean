/-
  must/0009-no-shell-scripts の Lean 版。uniqnode の条項には UUID の anchor が無い。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  追跡されるファイルと、shell に届く文字列を 2 つの主題にし、shell script を入れないことと
  shell の制御の流れを文字列で組まないことを Prop にする。
-/
namespace NoShellScripts

/-- このリポジトリが追跡する 1 つのファイル。 -/
structure TrackedFile where
  /-- git が追跡しているか。 -/
  tracked : Bool
  /-- 名前が `.sh` で終わるか。 -/
  shSuffix : Bool
  /-- 本体が sh か bash の系統の shebang で始まる shell の論理か。 -/
  shellShebang : Bool

/-- 適用のテスト。追跡されるファイル。 -/
def Applies (f : TrackedFile) : Prop := f.tracked = true

/-- 要求。`.sh` のファイルも、shell script を本体とするファイルも入れない。 -/
def Complies (f : TrackedFile) : Prop :=
  Applies f -> f.shSuffix = false /\ f.shellShebang = false

/-- このリポジトリが出荷する、shell に届く 1 つの文字列 (`sh -c`、broker の exec、nspawn や systemd-run の command)。 -/
structure ShellString where
  /-- 追跡されるコードの中に書かれた文字列か。エージェントが実行時に書く `run` のコマンドは外。 -/
  checkedIn : Bool
  /-- shell に届くか。`std::process::Command` で直接 argv を渡すものは外。 -/
  reachesShell : Bool
  /-- `&&`、`||`、`;` で 2 つ目のコマンドをつないでいるか。 -/
  sequencing : Bool
  /-- `if`、`for`、`while`、`until`、`case` を含むか。 -/
  controlFlow : Bool
  /-- `$( )` のコマンド置換を含むか。 -/
  commandSubstitution : Bool
  /-- パイプの本数。データの通り道としての 1 本だけが許される。引用、リダイレクト、builtin は数えない。 -/
  pipeCount : Nat

/-- 適用のテスト。追跡されるコードの中の、shell に届く文字列。uniqnode でも同じ 2 つの自己テストが守る。 -/
def ShellStringApplies (s : ShellString) : Prop :=
  s.checkedIn = true /\ s.reachesShell = true

/-- 要求。文字列は 1 つのコマンドであり、順序付け、分岐、繰り返し、置換を含まず、パイプは 1 本まで。 -/
def ShellStringComplies (s : ShellString) : Prop :=
  ShellStringApplies s ->
    s.sequencing = false /\ s.controlFlow = false /\ s.commandSubstitution = false /\
    s.pipeCount <= 1

/-- 追跡される `.sh` のファイルは要求を満たさない。 -/
theorem tracked_sh_file_fails (shebang : Bool) :
    Not (Complies { tracked := true, shSuffix := true, shellShebang := shebang }) := by
  intro h
  have e : true = false := (h rfl).1
  cases e

/-- `&&` で 2 つ目のコマンドをつないだ文字列は、要求を満たさない。 -/
theorem sequencing_fails (pipes : Nat) :
    Not (ShellStringComplies { checkedIn := true, reachesShell := true, sequencing := true,
                               controlFlow := false, commandSubstitution := false,
                               pipeCount := pipes }) := by
  intro h
  have e : true = false := (h (And.intro rfl rfl)).1
  cases e

/-- パイプを 2 本つないだ文字列は、要求を満たさない。 -/
theorem two_pipes_fail :
    Not (ShellStringComplies { checkedIn := true, reachesShell := true, sequencing := false,
                               controlFlow := false, commandSubstitution := false,
                               pipeCount := 2 }) := by
  intro h
  have e : 2 <= 1 := (h (And.intro rfl rfl)).2.2.2
  exact absurd e (by decide)

/-- 要求を満たす例。Rust のソースと宣言的な設定だけを追跡する木の中のファイル。 -/
theorem rust_source_complies :
    Complies { tracked := true, shSuffix := false, shellShebang := false } := by
  intro _
  exact And.intro rfl rfl

/-- 要求を満たす例。リダイレクトと 1 本のパイプを要するために `sh -c` で渡す 1 つのコマンド。 -/
theorem single_command_with_one_pipe_complies :
    ShellStringComplies { checkedIn := true, reachesShell := true, sequencing := false,
                          controlFlow := false, commandSubstitution := false,
                          pipeCount := 1 } := by
  intro _
  exact And.intro rfl (And.intro rfl (And.intro rfl (Nat.le_refl 1)))

/-- エージェントが実行時に書くコマンドには、条項が当たらない。 -/
theorem runtime_command_is_outside (seq flow subst : Bool) (pipes : Nat) :
    Not (ShellStringApplies { checkedIn := false, reachesShell := true, sequencing := seq,
                              controlFlow := flow, commandSubstitution := subst,
                              pipeCount := pipes }) := by
  intro h
  have e : false = true := And.left h
  cases e

end NoShellScripts
