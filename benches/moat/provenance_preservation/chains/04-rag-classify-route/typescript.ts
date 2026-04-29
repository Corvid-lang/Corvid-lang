// Vercel AI SDK RouterChain pattern. Classifier returns a string
// label; specialists return strings. The doc.id is not threaded.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc {
  return { id: "doc-7", text: `hit for ${query}` };
}

function classify(doc: Doc): string {
  return doc.text.includes("invoice") ? "billing" : "support";
}

function billingSpecialist(doc: Doc): string {
  return `billing reply for ${doc.text}`;
}

function supportSpecialist(doc: Doc): string {
  return `support reply for ${doc.text}`;
}

export function rag_classify_route(query: string): string {
  const doc = retrieve(query);
  const topic = classify(doc);
  if (topic === "billing") return billingSpecialist(doc);
  return supportSpecialist(doc);
}
