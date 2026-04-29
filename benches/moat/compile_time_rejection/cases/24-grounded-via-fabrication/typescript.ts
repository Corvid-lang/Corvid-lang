import { z } from "zod";

const Grounded = z.object({
  value: z.string(),
  sources: z.array(z.string()).default([]),
});
type Grounded = z.infer<typeof Grounded>;

function fabricate(seed: string): string {
  return `answer-${seed}`;
}

function helper(seed: string): string {
  return fabricate(seed);
}

export function answer(seed: string): Grounded {
  // BUG: empty sources; tsc passes.
  return Grounded.parse({ value: helper(seed), sources: [] });
}
