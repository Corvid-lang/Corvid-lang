import { z } from "zod";

const EvalAssertion = z.object({
  expr_truthy: z.boolean(),
  confidence: z.number(),
  runs: z.number().int(),
});

// BUG: zero-run assertion accepted by zod.
EvalAssertion.parse({ expr_truthy: true, confidence: 0.95, runs: 0 });
