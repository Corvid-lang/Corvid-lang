import { z } from "zod";

const EvalAssertion = z.object({
  expr_truthy: z.boolean(),
  confidence: z.number(),
  runs: z.number().int(),
});

// BUG: negative confidence accepted by zod.
EvalAssertion.parse({ expr_truthy: true, confidence: -0.1, runs: 5 });
