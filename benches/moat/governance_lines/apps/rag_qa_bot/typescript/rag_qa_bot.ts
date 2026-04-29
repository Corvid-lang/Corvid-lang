// RAG-QA-bot reference implementation — TypeScript.
//
// Same product as Corvid + Python: a bot that answers questions
// over an internal corpus and can share a source doc with the
// requester (the dangerous action). Every governance check is
// application code; tsc enforces shape, not policy.
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

const Question = z.object({
  user_id: z.string(),
  text: z.string(),
});
type Question = z.infer<typeof Question>;

const Answer = z.object({
  text: z.string(),
  sources: z.array(z.string()).default([]), // governance (provenance)
});
type Answer = z.infer<typeof Answer>;

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

const share_source = dangerous("human_required", function share_source(doc_id: string, user_id: string): string { // governance (decorator wrap)
  return `shared:${doc_id}:to:${user_id}`;
});

function retrieve_docs(query: string): Array<{ text: string; doc_id: string }> { // governance (return shape)
  // governance: must return text + doc_id pairs so callers preserve citations.
  return [ // governance
    { text: `doc-1 hit for ${query}`, doc_id: "kb://policies/1" }, // governance
    { text: `doc-2 hit for ${query}`, doc_id: "kb://policies/2" }, // governance
  ]; // governance
}

function synthesize(question: string, docs: Array<{ text: string; doc_id: string }>): { text: string; sources: string[] } {
  return { // governance (sources thread)
    text: `answer to '${question}' using ${docs.length} sources`, // governance
    sources: docs.map((d) => d.doc_id), // governance
  }; // governance
}

const budgetUsd: Record<string, number> = {}; // governance (per-user budget cap)

export function answer_question(q: Question): Answer {
  const spent = budgetUsd[q.user_id] ?? 0; // governance
  if (spent + 0.10 > 10.0) { // governance (budget cap)
    throw new Error(`budget exceeded for ${q.user_id}`); // governance
  } // governance
  budgetUsd[q.user_id] = spent + 0.10; // governance
  const docs = retrieve_docs(q.text);
  const { text, sources } = synthesize(q.text, docs);
  if (sources.length === 0) { // governance (catches dropped citation)
    throw new Error("ungrounded answer rejected"); // governance
  } // governance
  return { text, sources };
}

export function share_source_doc(doc_id: string, user_id: string): string {
  const approval: Approval = { trust: "human_required", actor: "human" }; // governance
  return share_source(approval, doc_id, user_id);
}
