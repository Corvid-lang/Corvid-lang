import { z } from "zod";

const Cost = z.object({ usd: z.number() });

function classify(text: string): string {
  return text;
}

function summarise(text: string): string {
  return text;
}

export function pipeline(text: string): string {
  // BUG: cumulative cost ($0.60) > intended ceiling ($0.50); tsc passes.
  return summarise(classify(text));
}
