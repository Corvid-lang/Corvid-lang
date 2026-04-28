// TypeScript equivalent — passes tsc --strict + zod. Effect row drift ships.

import { z } from "zod";

const Receipt = z.object({ id: z.string() });
type Receipt = z.infer<typeof Receipt>;

// Effects are convention-only in TS. A team might use a typed `Effect`
// union or a runtime decorator, but no static checker enforces that a
// caller's declared effect set covers its callees'.
type TrustLevel = "autonomous" | "supervisor_required" | "human_required";

function issue_refund(
  order_id: string,
  amount: number,
  trust: TrustLevel = "human_required",
): Receipt {
  return Receipt.parse({ id: `r-${order_id}` });
}

function helper(
  order_id: string,
  amount: number,
  trust: TrustLevel = "human_required",
): Receipt {
  return issue_refund(order_id, amount, trust);
}

// BUG: outer declares trust = "autonomous" but the helper it calls
// requires "human_required". tsc --strict passes. zod passes. The
// drift ships.
export function outer(
  order_id: string,
  amount: number,
  trust: TrustLevel = "autonomous",
): Receipt {
  return helper(order_id, amount);
}
