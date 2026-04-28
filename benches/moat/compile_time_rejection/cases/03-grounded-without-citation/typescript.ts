// TypeScript equivalent — passes tsc --strict + zod. Citation forgery ships.

import { z } from "zod";

// A "Grounded" wrapper is convention-only in TS. Anyone can construct
// one without a real source; tsc has no way to track provenance flow.
const Grounded = z.object({
  value: z.string(),
  sources: z.array(z.string()).default([]),
});
type Grounded = z.infer<typeof Grounded>;

function fabricate(seed: string): string {
  return `answer-for-${seed}`;
}

// BUG: returns a Grounded with empty sources. The type checker is
// satisfied; the answer claims to be grounded but cites nothing.
export function answer(seed: string): Grounded {
  return Grounded.parse({ value: fabricate(seed), sources: [] });
}
