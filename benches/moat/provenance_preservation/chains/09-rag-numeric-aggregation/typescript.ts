// Vercel AI SDK generateObject + array reduce. Per-doc
// extraction returns numbers; the sum is a number with no
// source binding.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc[] {
  return [0, 1, 2].map((i) => ({
    id: `doc-${i}`,
    text: `hit-${i} for ${query} costs $${10 * i + 5}`,
  }));
}

function extractAmount(doc: Doc): number {
  return doc.text.length % 100;
}

function sumAmounts(values: number[]): number {
  return values.reduce((a, b) => a + b, 0);
}

export function rag_numeric_aggregation(query: string): number {
  const docs = retrieve(query);
  const amounts = docs.map(extractAmount);
  return sumAmounts(amounts);
}
