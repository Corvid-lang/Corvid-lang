import { z } from "zod";

const Grounded = z.object({
  value: z.string(),
  sources: z.array(z.string()).default([]),
});
type Grounded = z.infer<typeof Grounded>;

function ask_model(q: string): string {
  return `likely answer for ${q}`;
}

export function answer(q: string): Grounded {
  // BUG: ask_model isn't a retrieval tool; sources empty.
  return Grounded.parse({ value: ask_model(q), sources: [] });
}
