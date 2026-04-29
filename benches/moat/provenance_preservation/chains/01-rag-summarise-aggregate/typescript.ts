// Vercel AI SDK + zod multi-hop RAG. After per-doc summarisation,
// the citation chain dissolves: the aggregator's typed schema
// returns a string, and there is no `sources` field surviving.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc[] {
  return [0, 1, 2].map((i) => ({ id: `doc-${i}`, text: `hit-${i} for ${query}` }));
}

function summarise(doc: Doc): string {
  // streamText / generateText returns string; doc.id is not threaded.
  return `summary of ${doc.text}`;
}

function aggregate(parts: string[]): string {
  // Aggregator receives strings; provenance gone.
  return parts.join("; ");
}

// Final return type is `string` — no typed sources field at the
// TS-level surface. A consumer cannot recover doc IDs without
// re-running step 1 or threading metadata manually.
export function multi_hop_rag(query: string): string {
  const docs = retrieve(query);
  const summaries = docs.map(summarise);
  return aggregate(summaries);
}
