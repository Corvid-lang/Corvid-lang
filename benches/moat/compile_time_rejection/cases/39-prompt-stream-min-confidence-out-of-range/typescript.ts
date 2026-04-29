import { z } from "zod";

const StreamConfig = z.object({ min_confidence: z.number() });

// BUG: out-of-range threshold accepted by zod.
StreamConfig.parse({ min_confidence: 1.5 });
