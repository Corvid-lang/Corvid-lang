import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, approval: Approval, reversible = false): number {
  return 1;
}

export function bot(id: string, declaredReversible = true): number {
  // BUG: declared reversibility wider than tool's actual semantics.
  return issue_refund(id, { label: "IssueRefund" });
}
