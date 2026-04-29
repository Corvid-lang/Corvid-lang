import { z } from "zod";

const EvalAssertion = z.object({
  expr_truthy: z.boolean(),
  confidence: z.number(),
  runs: z.number().int(),
});

// BUG: confidence > 1.0 accepted by zod (no range constraint).
EvalAssertion.parse({ expr_truthy: true, confidence: 1.5, runs: 5 });
