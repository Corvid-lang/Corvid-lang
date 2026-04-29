import { z } from "zod";

const Prompt = z.object({ template: z.string(), cites_strictly: z.string() });

function summarise(question: string, ctx: string): string {
  return `answer ${question} using ${ctx}`;
}

// BUG: cites a non-grounded param; zod does not check.
Prompt.parse({ template: "answer {question} using {ctx}", cites_strictly: "question" });
