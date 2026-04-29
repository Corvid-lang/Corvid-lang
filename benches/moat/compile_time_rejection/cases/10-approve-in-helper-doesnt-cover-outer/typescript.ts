import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, approval?: Approval): number {
  if (!approval) throw new Error("issue_refund requires approval");
  return 1;
}

function helper(id: string): number {
  return issue_refund(id, { label: "IssueRefund" });
}

export function outer(id: string): number {
  const a = helper(id);
  // BUG: missing runtime guard at the outer site.
  const b = issue_refund(id);
  return a + b;
}
