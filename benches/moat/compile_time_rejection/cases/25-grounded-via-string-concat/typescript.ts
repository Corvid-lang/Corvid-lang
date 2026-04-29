import { z } from "zod";

const Grounded = z.object({
  value: z.string(),
  sources: z.array(z.string()).default([]),
});
type Grounded = z.infer<typeof Grounded>;

export function answer(seed: string): Grounded {
  // BUG: built by string concat; no source.
  return Grounded.parse({ value: "answer-for-" + seed, sources: [] });
}
