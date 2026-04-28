// TypeScript equivalent — passes tsc --strict + zod. The bug ships.

import { z } from "zod";

const Receipt = z.object({
  id: z.string(),
});
type Receipt = z.infer<typeof Receipt>;

const RefundRequest = z.object({
  order_id: z.string(),
  amount: z.number().positive(),
});
type RefundRequest = z.infer<typeof RefundRequest>;

// Tool annotated "dangerous" via a JSDoc — there is no compile-time
// representation TypeScript+zod can enforce. A code-review convention
// might catch unapproved calls; the type system does not.
/** DANGEROUS: financial impact, irreversible. Requires human approval. */
function issue_refund(order_id: string, amount: number): Receipt {
  return Receipt.parse({ id: `r-${order_id}` });
}

export function refund_bot(req: RefundRequest): Receipt {
  // BUG: dangerous tool called without an approval check.
  // tsc --strict accepts this. zod accepts this. The bug ships.
  return issue_refund(req.order_id, req.amount);
}
