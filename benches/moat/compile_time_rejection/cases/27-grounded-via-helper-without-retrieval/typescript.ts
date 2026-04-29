import { z } from "zod";

const Grounded = z.object({
  value: z.string(),
  sources: z.array(z.string()).default([]),
});
type Grounded = z.infer<typeof Grounded>;

function search(q: string): string {
  return `hit for ${q}`;
}

function strip_provenance(q: string): string {
  return search(q);
}

export function answer(q: string): Grounded {
  // BUG: provenance dropped at the helper boundary; tsc passes.
  return Grounded.parse({ value: strip_provenance(q), sources: [] });
}
