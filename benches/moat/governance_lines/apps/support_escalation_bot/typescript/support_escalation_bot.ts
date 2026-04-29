// Support-escalation-bot reference implementation — TypeScript.
//
// Same product as Corvid + Python: triage tickets, page on-call
// for severe ones. Paging is dangerous (irreversible). Triage
// rationale must carry citations back to ticket history.
//
// Lines marked `// governance` are what the counter classifies.

import { z } from "zod"; // governance

const auditLog: Array<Record<string, unknown>> = []; // governance

const TrustLevel = z.enum([ // governance
  "autonomous", // governance
  "supervisor", // governance
  "human_required", // governance
]); // governance
type TrustLevel = z.infer<typeof TrustLevel>; // governance

const Ticket = z.object({
  id: z.string(),
  customer_id: z.string(),
  body: z.string(),
  severity: z.string(),
});
type Ticket = z.infer<typeof Ticket>;

const Triage = z.object({
  decision: z.string(),
  rationale: z.string(),
  sources: z.array(z.string()).default([]), // governance (provenance)
});
type Triage = z.infer<typeof Triage>;

interface Approval { // governance
  trust: TrustLevel; // governance
  actor: string; // governance
} // governance

function dangerous<TArgs extends unknown[], TRet>( // governance
  required: TrustLevel, // governance
  fn: (...args: TArgs) => TRet, // governance
): (approval: Approval, ...args: TArgs) => TRet { // governance
  return (approval, ...args) => { // governance
    if (!approval) { // governance
      throw new Error("dangerous tool requires an approval token"); // governance
    } // governance
    if (approval.trust !== required) { // governance
      throw new Error("approval trust level mismatch"); // governance
    } // governance
    auditLog.push({ // governance
      tool: fn.name, // governance
      approval, // governance
      ts: Date.now(), // governance
    }); // governance
    return fn(...args); // governance
  }; // governance
} // governance

const escalate_to_oncall = dangerous("human_required", function escalate_to_oncall(ticket_id: string, severity: string): string { // governance (decorator wrap)
  return `paged:${ticket_id}:sev=${severity}`;
});

function fetch_history(customer_id: string): Array<{ text: string; source_id: string }> { // governance (return shape)
  // governance: must return text + source_id pairs so callers preserve citations.
  return [ // governance
    { text: `prior ticket A for ${customer_id}`, source_id: `db://tickets/${customer_id}/a` }, // governance
    { text: `prior ticket B for ${customer_id}`, source_id: `db://tickets/${customer_id}/b` }, // governance
  ]; // governance
}

function classify_severity(body: string, history: Array<{ text: string; source_id: string }>): { decision: string; sources: string[] } {
  const decision = body.toLowerCase().includes("outage") ? "high" : "normal";
  const sources = history.map((h) => h.source_id); // governance (sources thread)
  return { decision, sources };
}

const budgetUsd: Record<string, number> = {}; // governance (per-customer budget cap)
const escalationsPerHour: Record<string, number> = {}; // governance (rate limit)

export function triage_ticket(t: Ticket): Triage {
  const spent = budgetUsd[t.customer_id] ?? 0; // governance
  if (spent + 0.10 > 5.0) { // governance (budget cap)
    throw new Error(`budget exceeded for ${t.customer_id}`); // governance
  } // governance
  budgetUsd[t.customer_id] = spent + 0.10; // governance
  const history = fetch_history(t.customer_id);
  const { decision, sources } = classify_severity(t.body, history);
  if (sources.length === 0) { // governance (catches dropped citation)
    throw new Error("ungrounded triage rejected"); // governance
  } // governance
  return { decision, rationale: decision, sources };
}

export function escalate(t: Ticket, severity: string): string {
  const count = escalationsPerHour[t.customer_id] ?? 0; // governance
  if (count >= 5) { // governance (rate limit)
    throw new Error("escalation rate exceeded"); // governance
  } // governance
  escalationsPerHour[t.customer_id] = count + 1; // governance
  const approval: Approval = { trust: "human_required", actor: "human" }; // governance
  return escalate_to_oncall(approval, t.id, severity);
}
