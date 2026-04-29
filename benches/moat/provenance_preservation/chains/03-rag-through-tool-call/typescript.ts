// Vercel AI SDK Tool-augmented RAG. The tool call drops the source
// binding when it returns a string; the answer has no way to trace
// back to the original retrieval.

import { z } from "zod";

const Doc = z.object({ id: z.string(), text: z.string() });
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc {
  return { id: "doc-0", text: `primary doc for ${query}` };
}

function enrich_from_db(doc: Doc): string {
  // Tool returns a string; doc.id is dropped.
  return `related-record-for-${doc.text}`;
}

function answer(doc: Doc, related: string): string {
  // generateText returns string; sources gone.
  return `answer using ${doc.text} and related ${related}`;
}

// Final return type is `string` — no typed sources field at the
// TS-level surface.
export function retrieve_enrich_answer(query: string): string {
  const doc = retrieve(query);
  const related = enrich_from_db(doc);
  return answer(doc, related);
}
