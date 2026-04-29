import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, amount: number, approval?: Approval): string {
  if (!approval || approval.label !== "IssueRefund") {
    throw new Error("approval label mismatch");
  }
  return `r-${id}`;
}

export function bot(id: string, amount: number): string {
  // BUG: label mismatch passes tsc.
  return issue_refund(id, amount, { label: "RefundIssue" });
}
