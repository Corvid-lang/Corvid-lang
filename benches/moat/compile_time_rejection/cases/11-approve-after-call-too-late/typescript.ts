import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, approval?: Approval): number {
  if (!approval) throw new Error("issue_refund requires approval");
  return 1;
}

export function bot(id: string): number {
  // BUG: trailing approve ineffective; tsc passes.
  const value = issue_refund(id);
  Approval.parse({ label: "IssueRefund" });
  return value;
}
