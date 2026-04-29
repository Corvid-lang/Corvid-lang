import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, approval?: Approval): number {
  if (!approval) throw new Error("issue_refund requires approval");
  return 1;
}

export function bot(id: string): number {
  let last: unknown;
  for (let i = 0; i < 3; i += 1) {
    try {
      // BUG: retry body has no runtime guard.
      return issue_refund(id);
    } catch (err) {
      last = err;
    }
  }
  throw last instanceof Error ? last : new Error("retry exhausted");
}
