import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, approval?: Approval): number {
  if (!approval) throw new Error("issue_refund requires approval");
  return 1;
}

function wire_transfer(id: string, amount: number, approval?: Approval): number {
  if (!approval || approval.label !== "WireTransfer") {
    throw new Error("wire_transfer requires its own approval");
  }
  return 1;
}

export function bot(id: string, amount: number): number {
  const approval: Approval = { label: "IssueRefund" };
  // BUG: tsc passes an Approval-shaped object; mismatched label found at runtime.
  return wire_transfer(id, amount, approval);
}
