/-
  should/0120-no-blocking-on-client-held-resources の Lean 版。
  条項の本文は同じ名前の .md にあり、食い違ったら .md が優先する。
  別の活動の完了の経路に置かれた 1 つの操作を主題にし、同じ類の活動が持つ資源を待って塞がらず、
  無期限に待たないことを Prop にする。
-/
namespace NoBlockingOnClientHeldResources

/-- 操作が資源をどう得るか。好ましい順に並ぶ。 -/
inductive Strategy where
  /-- 仕える活動が既に持つ割り当てを使い回す。 -/
  | reuseServedAllocation
  /-- 本当に空いている資源を、期限付きの貸与で機会的に取る。 -/
  | boundedLeaseOnIdle
  /-- 劣化して進む。共有する、後ろに並ぶ、予約を省く。 -/
  | proceedDegraded
  /-- 同じ類の活動が持つ資源が空くのを無期限に待つ。 -/
  | waitIndefinitely

/-- 別の活動の完了の経路に置かれた操作 (検証、採点、後片付け、確定)。 -/
structure Operation where
  /-- 活動が終わるために通らなければならない操作か。 -/
  onCompletionPath : Bool
  /-- 限られた資源を消費するか。 -/
  consumesLimitedResource : Bool
  /-- 資源の得かた。 -/
  strategy : Strategy

/-- 適用のテスト。完了の経路にあり、限られた資源を消費する操作。 -/
def Applies (o : Operation) : Prop :=
  o.onCompletionPath = true /\ o.consumesLimitedResource = true

/-- 要求。無期限に待たない。完了の経路では、生存が厳密な資源の勘定より優先する。 -/
def Complies (o : Operation) : Prop :=
  Applies o -> Not (o.strategy = Strategy.waitIndefinitely)

/-- 「空いたサーバーを待つ」検証器は、全てのサーバーを検証待ちの作業者が持った瞬間に止まり、
    要求を満たさない。 -/
theorem verifier_waiting_for_free_server_fails :
    Not (Complies { onCompletionPath := true, consumesLimitedResource := true,
                    strategy := Strategy.waitIndefinitely }) := by
  intro h
  exact h (And.intro rfl rfl) rfl

/-- 要求を満たす例。依頼元の割り当てに乗る検証器。循環は構成で消える。 -/
theorem riding_requester_allocation_complies :
    Complies { onCompletionPath := true, consumesLimitedResource := true,
               strategy := Strategy.reuseServedAllocation } := by
  intro _ e
  cases e

/-- 要求を満たす例。空いた資源を期限付きで取る操作。 -/
theorem bounded_lease_complies :
    Complies { onCompletionPath := true, consumesLimitedResource := true,
               strategy := Strategy.boundedLeaseOnIdle } := by
  intro _ e
  cases e

/-- 完了の経路に無い操作は対象外であり、何も求められない。 -/
theorem off_path_operation_complies (s : Strategy) :
    Complies { onCompletionPath := false, consumesLimitedResource := true, strategy := s } := by
  intro a
  cases And.left a

end NoBlockingOnClientHeldResources
