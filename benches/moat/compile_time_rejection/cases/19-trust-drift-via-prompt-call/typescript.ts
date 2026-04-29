import { z } from "zod";

type TrustLevel = "autonomous" | "human_required";

const Prompt = z.object({
  template: z.string(),
  trust: z.enum(["autonomous", "human_required"]),
});

function advise(q: string): string {
  return `advice for ${q}`;
}

export function ask(q: string, declaredTrust: TrustLevel = "autonomous"): string {
  // BUG: advise's trust dimension wider than declared; tsc passes.
  return advise(q);
}
