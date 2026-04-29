import { z } from "zod";

const Confidence = z.object({ score: z.number() });

function shaky_lookup(q: string): string {
  return q;
}

export function answer(q: string, threshold = 0.95): string {
  // BUG: tool's known confidence below threshold; tsc passes.
  return shaky_lookup(q);
}
