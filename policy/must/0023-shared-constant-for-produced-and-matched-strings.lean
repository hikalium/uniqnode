/-
  must/0023-shared-constant-for-produced-and-matched-strings の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。文字列を作る場所とそれを
  照合する場所の対を構造体で表し、1 つの名前付き定数を共有する要求を Prop にする。
-/
namespace SharedConstantForProducedAndMatchedStrings

/-- 文を組み立てる場所 (道具の観測、ゲートのマーク、拒否の文) と、その文を照合して何かを決める場所の
1 つの対。 -/
structure MarkerPair where
  /-- このリポジトリの Rust のソース (container/src/、container/tests/、container/build.rs) の
  中の対か。 -/
  inRustSource : Bool
  /-- 両方の場所が、1 つの名前付き定数を参照する (リテラルを両方に綴らない) か。 -/
  sharedConstant : Bool
  /-- その定数が作る側に置かれ、マークする物の名前で呼ばれるか。 -/
  constantLivesWithProducer : Bool
  /-- 作る側の経路で本物の観測を描き、照合する側がそれを認識することを検証するテストがあるか。
  定数を定数自身と照合するテストは数えない。 -/
  renderingTestThroughProducer : Bool

/-- 適用のテスト。Rust のソースの中で、一方が作り他方が照合する文字列の対。 -/
def Applies (p : MarkerPair) : Prop :=
  p.inRustSource = true

/-- リテラルを両方の場所に綴らず、1 つの定数を参照する。 -/
def OneConstant (p : MarkerPair) : Prop :=
  p.sharedConstant = true

/-- 定数は作る側に住む。 -/
def ConstantWithProducer (p : MarkerPair) : Prop :=
  p.constantLivesWithProducer = true

/-- 対は、作る経路を通す描画のテストを持つ。 -/
def RenderingTest (p : MarkerPair) : Prop :=
  p.renderingTestThroughProducer = true

/-- 要求。条項が当たる対は、1 つの定数を共有し、描画のテストで留められている。 -/
def Complies (p : MarkerPair) : Prop :=
  Applies p -> OneConstant p /\ ConstantWithProducer p /\ RenderingTest p

/-- 両方の場所にリテラルを綴った対は、一方の言い換えで黙って判定が裏返るので要求を満たさない。 -/
theorem literal_at_both_sites_fails (withProducer tested : Bool) :
    Not (Complies { inRustSource := true, sharedConstant := false,
                    constantLivesWithProducer := withProducer,
                    renderingTestThroughProducer := tested }) := by
  intro h
  cases (h rfl).1

/-- 定数を定数自身と照合するだけのテストしか無い対は、要求を満たさない。 -/
theorem self_matching_test_fails (withProducer : Bool) :
    Not (Complies { inRustSource := true, sharedConstant := true,
                    constantLivesWithProducer := withProducer,
                    renderingTestThroughProducer := false }) := by
  intro h
  cases (h rfl).2.2

/-- 要求を満たす例。verify::SUBSTANCE_FLOOR_MARK を作る側と分類器が共有し、本物の観測を描く
テストで留めた対。 -/
theorem shared_marker_constant_complies :
    Complies { inRustSource := true, sharedConstant := true,
               constantLivesWithProducer := true, renderingTestThroughProducer := true } := by
  intro _
  exact And.intro rfl (And.intro rfl rfl)

end SharedConstantForProducedAndMatchedStrings
