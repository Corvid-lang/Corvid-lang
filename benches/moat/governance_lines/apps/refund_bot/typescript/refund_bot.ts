// Refund-bot reference implementation — TypeScript.
//
// Every governance check is application code: a runtime guard, a zod
// schema validation, a manual audit-log push, a `sources: string[]`
// field threaded through every call. tsc enforces shape, not policy.
//
// Lines marked with `// governance` are the ones the counter classifies.

import { z } from "zod"; // governance

// governance: in-memory audit log; convention-only.
const auditLog: Array<Record<string, unknown>> = []; // governance

const TrustLevel = z.enum([ // governance
  "autonomous", // governance
  "supervisor", // governance
  "human_required", // governance
]); // governance
type TrustLevel = z.infer<typeof TrustLevel>; // governance

const RefundRequest = z.object({
  order_id: z.string(),
  amount: z.number().positive().max(500), // governance (budget cap)
  reason: z.string(),
});
type RefundRequest = z.infer<typeof RefundRequest>;

const RefundResponse = z.object({
  receipt_id: z.string(),
  status: z.string(),
});
type RefundResponse = z.infer<typeof RefundResponse>;

const RefundExplanation = z.object({
  reason: z.string(),
  sources: z.array(z.string()).default([]), // governance (provenance)
});
type RefundExplanation = z.infer<typeof RefundExplanation>;

interface Approval { // governance
  trust: TrustLevel; // governance
  actor: string; // governance
} // governance

function dangerous<TArgs extends unknown[], TRet>( // governance
  required: TrustLevel, // governance
  fn: (...args: TArgs) => TRet, // governance
): (approval: Approval, ...args: TArgs) => TRet { // governance
  return (approval, ...args) => { // governance
    if (!approval) { // governance
      throw new Error("dangerous tool requires an approval token"); // governance
    } // governance
    if (approval.trust !== required) { // governance
      throw new Error("approval trust level mismatch"); // governance
    } // governance
    auditLog.push({ // governance
      tool: fn.name, // governance
      approval, // governance
      ts: Date.now(), // governance
    }); // governance
    return fn(...args); // governance
  }; // governance
} // governance

const issue_refund = dangerous("human_required", function issue_refund(req: RefundRequest): string { // governance (decorator wrap)
  return `r-${req.order_id}`;
});

function fetch_order(order_id: string): { text: string; sources: string[] } { // governance (return shape)
  // governance: must return text + sources so callers can preserve citations.
  return { // governance
    text: `order ${order_id} placed 2026-04-21`, // governance
    sources: [`db://orders/${order_id}`], // governance
  }; // governance
}

export function approve_refund(req: RefundRequest): RefundResponse {
  const approval: Approval = { trust: "human_required", actor: "human" }; // governance
  const receipt_id = issue_refund(approval, req);
  return { receipt_id, status: "approved" };
}

export function explain_refund(order_id: string): RefundExplanation {
  const { text, sources } = fetch_order(order_id);
  if (sources.length === 0) { // governance (catches dropped citation)
    throw new Error("ungrounded explanation rejected"); // governance
  } // governance
  return { reason: text, sources };
}
