// Vercel AI SDK extraction + translation. The extracted entities
// lose their source binding once they are strings.

import { z } from "zod";

const Doc = z.object({ id: z.string(), text: z.string() });
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc {
  return { id: "doc-0", text: `news article about ${query}` };
}

function extract_entities(doc: Doc): string[] {
  // structured output schema returns string[]; doc.id dropped.
  return [0, 1, 2].map((i) => `entity-${i}-from-${doc.text}`);
}

function translate(entity: string): string {
  return `es:${entity}`;
}

// Final return type is string[] — no typed sources field.
export function extract_then_translate(query: string): string[] {
  const doc = retrieve(query);
  const entities = extract_entities(doc);
  return entities.map(translate);
}
