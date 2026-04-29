import { z } from "zod";

const StageConfig = z.object({ model: z.string(), threshold: z.number() });

// BUG: out-of-range threshold accepted by zod.
StageConfig.parse({ model: "fast", threshold: 1.2 });
