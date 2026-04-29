// Vercel AI SDK + custom rerank by score. Top-k are strings;
// doc IDs of surviving items are dropped.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc[] {
  return [0, 1, 2, 3, 4, 5, 6, 7].map((i) => ({
    id: `doc-${i}`,
    text: `hit-${i} for ${query}`,
  }));
}

function rerankTopK(items: Doc[], k: number): string[] {
  const scored = [...items].sort((a, b) => b.text.length - a.text.length);
  return scored.slice(0, k).map((d) => d.text);
}

export function rag_rerank_top_k(query: string, k: number): string[] {
  const candidates = retrieve(query);
  return rerankTopK(candidates, k);
}
