// Vercel AI SDK + Set-based dedupe. Surviving entries are
// strings; source IDs vanish at the dedupe step.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

function retrieveWiki(query: string): Doc[] {
  return [0, 1].map((i) => ({ id: `wiki-${i}`, text: `wiki-hit-${i} for ${query}` }));
}

function retrieveInternal(query: string): Doc[] {
  return [0, 1].map((i) => ({ id: `int-${i}`, text: `int-hit-${i} for ${query}` }));
}

function dedupe(items: Doc[]): string[] {
  return Array.from(new Set(items.map((d) => d.text)));
}

function aggregate(parts: string[]): string {
  return parts.join("; ");
}

export function multi_source_dedupe(query: string): string {
  const merged = dedupe([...retrieveWiki(query), ...retrieveInternal(query)]);
  return aggregate(merged);
}
