import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, approval?: Approval): number {
  if (!approval) throw new Error("issue_refund requires approval");
  return 1;
}

export function bot(id: string, debug: boolean): number {
  let approval: Approval | undefined;
  if (debug) approval = { label: "IssueRefund" };
  // BUG: undefined approval at runtime when debug is false.
  return issue_refund(id, approval);
}
