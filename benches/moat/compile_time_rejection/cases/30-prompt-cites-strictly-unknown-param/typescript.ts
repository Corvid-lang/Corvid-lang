import { z } from "zod";

const Prompt = z.object({
  template: z.string(),
  cites_strictly: z.string(),
});

function summarise(ctx: string): string {
  return `summarise: ${ctx}`;
}

const _metadata = Prompt.parse({
  template: "summarise: {ctx}",
  cites_strictly: "context",
});
