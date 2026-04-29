import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;
const Grounded = z.object({ value: z.string(), sources: z.array(z.string()).default([]) });
type Grounded = z.infer<typeof Grounded>;

function issue_refund(id: string, approval?: Approval): string {
  if (!approval) throw new Error("issue_refund requires approval");
  return `r-${id}`;
}

export function triage(id: string, budgetUsd = 0.10): Grounded {
  // BUG: 3 contract violations; tsc passes.
  return Grounded.parse({ value: issue_refund(id, { label: "x" }), sources: [] });
}
