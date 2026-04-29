import { z } from "zod";

const Grounded = z.object({
  value: z.string(),
  sources: z.array(z.string()).default([]),
});
type Grounded = z.infer<typeof Grounded>;

function opinion(q: string): string {
  return `my view on ${q}`;
}

export function answer(q: string): Grounded {
  // BUG: opinion has no provenance; sources empty; tsc passes.
  return Grounded.parse({ value: opinion(q), sources: [] });
}
