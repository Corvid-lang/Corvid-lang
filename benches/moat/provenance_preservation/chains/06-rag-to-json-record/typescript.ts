// Vercel AI SDK generateObject with a typed Product schema.
// The Product type lacks a sources field — typed answer, lost
// provenance.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

const Product = z.object({
  name: z.string(),
  price: z.number(),
  description: z.string(),
});
type Product = z.infer<typeof Product>;

function retrieve(query: string): Doc {
  return { id: "doc-3", text: `product blurb for ${query}` };
}

function extractProduct(doc: Doc): Product {
  return { name: "widget", price: 9.99, description: doc.text };
}

export function rag_to_json_record(query: string): Product {
  const doc = retrieve(query);
  return extractProduct(doc);
}
