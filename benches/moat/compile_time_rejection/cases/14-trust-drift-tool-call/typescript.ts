import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;
type TrustLevel = "autonomous" | "supervisor" | "human_required";

function issue_refund(
  id: string, approval: Approval, trust: TrustLevel = "human_required",
): number {
  return 1;
}

export function bot(id: string, declaredTrust: TrustLevel = "autonomous"): number {
  // BUG: tool's required trust narrower than declared; tsc accepts.
  return issue_refund(id, { label: "IssueRefund" });
}
