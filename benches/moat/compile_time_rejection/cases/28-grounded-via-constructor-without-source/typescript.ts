import { z } from "zod";

const Grounded = z.object({
  value: z.string(),
  sources: z.array(z.string()).default([]),
});
type Grounded = z.infer<typeof Grounded>;

export function answer(q: string): Grounded {
  // BUG: literal value with no sources; tsc passes.
  return Grounded.parse({ value: "the sky is blue", sources: [] });
}
