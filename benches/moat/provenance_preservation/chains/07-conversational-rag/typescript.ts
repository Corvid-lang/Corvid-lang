// Vercel AI SDK streamText with messages array. The messages
// array holds plain strings; per-turn sources never reach the
// final return type.

import { z } from "zod";

const Doc = z.object({
  id: z.string(),
  text: z.string(),
});
type Doc = z.infer<typeof Doc>;

function retrieve(query: string): Doc {
  return { id: `doc-${Math.abs(query.length) % 100}`, text: `hit for ${query}` };
}

function reply(history: string[], turn: Doc): string {
  return `reply given ${history.join(" | ")} and ${turn.text}`;
}

export function conversational_rag(history: string[], query: string): string {
  const historyText = history.map((h) => retrieve(h).text);
  const turnDoc = retrieve(query);
  return reply(historyText, turnDoc);
}
