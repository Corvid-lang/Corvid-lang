// Vercel AI SDK + a custom moderation guard. The guard takes a
// string and returns a string; doc-id origin is not preserved.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc {
  return { id: "doc-9", text: `raw text for ${query}` };
}

function safetyFilter(text: string): string {
  return text.replace("RAW", "[redacted]");
}

function summarise(text: string): string {
  return `summary of ${text}`;
}

export function rag_safety_filter(query: string): string {
  const raw = retrieve(query);
  const filtered = safetyFilter(raw.text);
  return summarise(filtered);
}
