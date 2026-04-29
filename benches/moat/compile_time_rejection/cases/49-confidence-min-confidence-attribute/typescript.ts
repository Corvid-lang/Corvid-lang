import { z } from "zod";

const EffectMeta = z.object({ name: z.string(), confidence: z.number() });

// BUG: out-of-range confidence accepted by zod.
EffectMeta.parse({ name: "bad_conf", confidence: 1.7 });
