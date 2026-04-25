const topics = [
  {
    name: "approve-gates",
    title: "Approve Before Dangerous",
    category: "Safety at compile time",
    pitch: "Dangerous actions are not library conventions. The compiler requires an explicit approval boundary before irreversible tools run.",
    spec: "../effects-spec/03-typing-rules.md",
    test: "crates/corvid-types/src/lib.rs approval checker tests",
    nonScope: "Does not decide whether a human should approve; it proves the approval boundary exists.",
    source: `type Receipt:
    id: String

tool issue_refund(id: String) -> Receipt dangerous

agent refund(id: String) -> Receipt:
    approve IssueRefund(id)
    return issue_refund(id)`
  },
  {
    name: "dimensional-effects",
    title: "Dimensional Effects",
    category: "Safety at compile time",
    pitch: "Effects are not flat tags. Cost, trust, reversibility, data, latency, confidence, and user dimensions compose with their own algebra.",
    spec: "../effects-spec/02-composition-algebra.md",
    test: "crates/corvid-types/src/effects.rs composition tests",
    nonScope: "Does not make external providers honest; it proves the declared Corvid contract.",
    source: `effect llm_call:
    cost: $0.05
    trust: autonomous

prompt summarize(text: String) -> String uses llm_call:
    "Summarize {text}"

@budget($0.10)
@trust(autonomous)
agent summarize_twice(text: String) -> String:
    first = summarize(text)
    return summarize(first)`
  },
  {
    name: "grounded-values",
    title: "Grounded<T> Provenance",
    category: "Safety at compile time",
    pitch: "A grounded return must flow from a retrieval source. Runtime values carry the provenance chain that made them grounded.",
    spec: "../effects-spec/05-grounding.md",
    test: "crates/corvid-types/src/effects/grounded.rs tests",
    nonScope: "Does not prove the retrieved document is true; it proves the answer is sourced.",
    source: `effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent research(id: String) -> Grounded<String>:
    return fetch_doc(id)`
  },
  {
    name: "strict-citations",
    title: "Strict Citation Contracts",
    category: "Safety at compile time",
    pitch: "A prompt can name the grounded context it must cite. Compile-time checks prove the parameter is grounded; runtime checks the response.",
    spec: "../effects-spec/05-grounding.md",
    test: "crates/corvid-vm/src/tests/dispatch.rs citation tests",
    nonScope: "Strict citation checks text evidence; it does not judge source truth.",
    source: `effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

prompt answer(ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    "Answer from {ctx}"

agent cited(id: String) -> Grounded<String>:
    return answer(fetch_doc(id))`
  },
  {
    name: "cost-budgets",
    title: "Compile-Time Budgets",
    category: "Safety at compile time",
    pitch: "Budget annotations are static constraints over the composed cost tree, not billing dashboards after the model call has run.",
    spec: "../effects-spec/07-cost-budgets.md",
    test: "crates/corvid-types/src/effects/cost.rs tests",
    nonScope: "Static budgets use declared costs; provider invoices still need operational reconciliation.",
    source: `effect cheap_call:
    cost: $0.05

prompt classify(text: String) -> String uses cheap_call:
    "Classify {text}"

@budget($0.10)
agent bounded(text: String) -> String:
    first = classify(text)
    return classify(first)`
  },
  {
    name: "confidence-gates",
    title: "Confidence As A Dimension",
    category: "Safety at compile time",
    pitch: "Confidence composes by weakest link. Agents can require a floor, and trust gates can escalate when confidence drops.",
    spec: "../effects-spec/06-confidence-gates.md",
    test: "crates/corvid-types/src/tests.rs min_confidence tests",
    nonScope: "Confidence is only meaningful when model adapters provide calibrated signals.",
    source: `effect llm_decision:
    confidence: 0.95

tool search(query: String) -> String uses llm_decision

@min_confidence(0.90)
agent bot(query: String) -> String:
    return search(query)`
  },
  {
    name: "eval-traces",
    title: "Trace-Aware Evals",
    category: "AI-native ergonomics",
    pitch: "Evals can assert process, not just output. Corvid checks whether the agent called, approved, ordered, and spent as intended.",
    spec: "../effects-spec/12-verification.md",
    test: "crates/corvid-types/src/lib.rs eval assertion tests",
    nonScope: "This is language and checker support; the full eval runner is later workflow tooling.",
    source: `agent always_refund() -> Bool:
    return true

eval refund_accuracy:
    result = always_refund()
    assert result == true`
  },
  {
    name: "language-keywords",
    title: "AI-Native Keywords",
    category: "AI-native ergonomics",
    pitch: "Agents, tools, prompts, effects, approvals, models, evals, and streams are syntax the compiler can reason about.",
    spec: "../effects-spec/01-dimensional-syntax.md",
    test: "crates/corvid-syntax/src/parser/tests.rs",
    nonScope: "Keywords do not replace ordinary general-purpose code; they make AI boundaries visible.",
    source: `model local:
    capability: basic

prompt say(name: String) -> String:
    requires: basic
    "Hello {name}"

agent hello(name: String) -> String:
    return say(name)`
  },
  {
    name: "streaming-effects",
    title: "Streaming Effects",
    category: "Streaming",
    pitch: "Streams are typed values that carry effects mid-flight. Budgets, confidence, provenance, and backpressure are not after-the-fact logs.",
    spec: "../effects-spec/08-streaming.md",
    test: "crates/corvid-vm/src/tests/stream.rs",
    nonScope: "Provider-native continuation depends on provider APIs; local typed fallback tokens are the shipped boundary.",
    source: `agent count() -> Stream<Int>:
    yield 1
    yield 2`
  },
  {
    name: "partial-streams",
    title: "Progressive Structured Streams",
    category: "Streaming",
    pitch: "Partial<T> lets the program read complete fields as they arrive while the rest of a structured stream is still forming.",
    spec: "../effects-spec/08-streaming.md",
    test: "crates/corvid-types/src/tests.rs partial stream tests",
    nonScope: "Native codegen support for every partial-stream path is still bounded by backend parity work.",
    source: `type Plan:
    title: String
    body: String

agent read(snapshot: Partial<Plan>) -> Option<String>:
    return snapshot.title`
  },
  {
    name: "stream-resume",
    title: "Typed Stream Resumption",
    category: "Streaming",
    pitch: "A ResumeToken<T> captures the typed stream element contract, so continuation cannot resume the wrong prompt shape.",
    spec: "../effects-spec/08-streaming.md",
    test: "crates/corvid-vm/src/tests/stream.rs resume tests",
    nonScope: "Provider-native session continuation waits on provider APIs; local fallback is shipped.",
    source: `prompt draft(topic: String) -> Stream<String>:
    "Draft {topic}"

agent capture(topic: String) -> ResumeToken<String>:
    stream = draft(topic)
    return resume_token(stream)`
  },
  {
    name: "stream-fanout",
    title: "Declarative Fan-Out / Fan-In",
    category: "Streaming",
    pitch: "Streams can split by structured fields and merge back with deterministic ordering, preserving typed stream effects.",
    spec: "../effects-spec/08-streaming.md",
    test: "crates/corvid-types/src/tests.rs stream_split_merge_ordered_by_typechecks",
    nonScope: "Field-keyed split is shipped; first-class lambda extractors wait for function values.",
    source: `type Event:
    kind: String
    body: String

agent source() -> Stream<Event>:
    yield Event("b", "two")
    yield Event("a", "one")

agent fanout() -> Stream<Event>:
    groups = source().split_by("kind")
    return merge(groups).ordered_by("fair_round_robin")`
  },
  {
    name: "model-routing",
    title: "Typed Model Routing",
    category: "Adaptive routing",
    pitch: "Models are typed declarations with capability and policy dimensions. Prompt dispatch is checked against those model contracts.",
    spec: "../effects-spec/13-model-substrate-shipped.md",
    test: "crates/corvid-vm/src/tests/dispatch.rs",
    nonScope: "Does not benchmark model quality automatically; routing reports use recorded eval history.",
    source: `model fast:
    capability: basic

model deep:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == "hard" -> deep
        _ -> fast
    "Answer {q}"`
  },
  {
    name: "progressive-routing",
    title: "Progressive Refinement",
    category: "Adaptive routing",
    pitch: "A prompt can try cheap models first and escalate only when confidence falls below a typed threshold.",
    spec: "../effects-spec/13-model-substrate-shipped.md#135-progressive-refinement-slice-e",
    test: "crates/corvid-vm/src/tests/dispatch.rs progressive tests",
    nonScope: "Thresholds are only meaningful when adapters report calibrated confidence.",
    source: `model cheap:
    capability: basic

model medium:
    capability: standard

model expensive:
    capability: expert

prompt classify(q: String) -> String:
    progressive:
        cheap below 0.80
        medium below 0.95
        expensive
    "Classify {q}"`
  },
  {
    name: "ensemble-voting",
    title: "Ensemble Voting",
    category: "Adaptive routing",
    pitch: "One prompt can dispatch to several models concurrently and fold the answers through a typed voting strategy.",
    spec: "../effects-spec/13-model-substrate-shipped.md#137-ensemble-voting-slice-f",
    test: "crates/corvid-vm/src/tests/dispatch.rs ensemble tests",
    nonScope: "Majority voting is shipped; arbitrary custom vote functions are future language work.",
    source: `model opus:
    capability: expert

model sonnet:
    capability: expert

model haiku:
    capability: standard

prompt classify(q: String) -> String:
    ensemble [opus, sonnet, haiku] vote majority
    "Classify {q}"`
  },
  {
    name: "privacy-routing",
    title: "Jurisdiction And Privacy Dimensions",
    category: "Adaptive routing",
    pitch: "Model selection can carry regulatory dimensions such as jurisdiction, compliance, and privacy tier as typed model facts.",
    spec: "../effects-spec/13-model-substrate-shipped.md#134-regulatory--compliance--privacy-dimensions-slice-d",
    test: "crates/corvid-types/src/effects.rs dimension law tests",
    nonScope: "The compiler enforces declared routing facts; legal compliance still requires operational controls.",
    source: `model eu_private:
    jurisdiction: eu_hosted
    compliance: gdpr
    privacy_tier: strict
    capability: expert`
  },
  {
    name: "replay-receipts",
    title: "Replay And Receipts",
    category: "Verification",
    pitch: "Executions become evidence. Traces, replay, trace-diff, and signed receipts turn behavior changes into reviewable artifacts.",
    spec: "../effects-spec/14-replay.md",
    test: "crates/corvid-cli/tests/bundle_verify.rs",
    nonScope: "A receipt is cryptographic evidence of observed behavior, not a formal proof of all possible runs.",
    source: `@deterministic
@replayable
agent classify(text: String) -> String:
    return text`
  },
  {
    name: "effect-registry",
    title: "Proof-Carrying Dimension Registry",
    category: "Verification",
    pitch: "Corvid can distribute pieces of the effect algebra as signed artifacts with law checks, proofs, and regression programs.",
    spec: "../effects-spec/dimension-artifacts.md",
    test: "crates/corvid-driver/src/dimension_registry.rs tests",
    nonScope: "The registry distributes declarations, not executable code or unverified trust.",
    source: `effect local_policy:
    data: pii
    reversible: true

tool read_profile(id: String) -> String uses local_policy

agent profile(id: String) -> String:
    return read_profile(id)`
  },
  {
    name: "adversarial-tests",
    title: "Adversarial Bypass Testing",
    category: "Verification",
    pitch: "The compiler ships with a bypass-attempt taxonomy so AI can attack Corvid's own effect system in CI.",
    spec: "../effects-spec/adversarial-taxonomy.md",
    test: "crates/corvid-driver/src/adversarial.rs tests",
    nonScope: "Live LLM generation expands the corpus; deterministic seeds remain the safety gate.",
    source: `effect transfer_money:
    trust: human_required
    reversible: false

tool refund(id: String) -> String dangerous uses transfer_money

@trust(human_required)
agent safe_refund(id: String) -> String:
    approve Refund(id)
    return refund(id)`
  }
];

const topicList = document.querySelector("#topic-list");
const category = document.querySelector("#topic-category");
const title = document.querySelector("#topic-title");
const pitch = document.querySelector("#topic-pitch");
const command = document.querySelector("#topic-command");
const source = document.querySelector("#topic-source");
const spec = document.querySelector("#topic-spec");
const test = document.querySelector("#topic-test");
const nonScope = document.querySelector("#topic-nonscope");
const copyCommand = document.querySelector("#copy-command");

let active = topics[0];

function runCommand(topic) {
  return `cargo run -q -p corvid-cli -- tour --topic ${topic.name}`;
}

function renderTopic(topic) {
  active = topic;
  category.textContent = topic.category;
  title.textContent = topic.title;
  pitch.textContent = topic.pitch;
  command.textContent = runCommand(topic);
  source.textContent = topic.source;
  spec.href = topic.spec;
  test.textContent = topic.test;
  nonScope.textContent = topic.nonScope;
  for (const button of topicList.querySelectorAll("button")) {
    button.classList.toggle("active", button.dataset.topic === topic.name);
  }
}

function copyText(text) {
  if (navigator.clipboard) {
    navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  textarea.remove();
}

for (const topic of topics) {
  const button = document.createElement("button");
  button.type = "button";
  button.dataset.topic = topic.name;
  button.textContent = `${topic.title} - ${topic.category}`;
  button.addEventListener("click", () => renderTopic(topic));
  topicList.appendChild(button);
}

document.querySelectorAll("[data-copy]").forEach((button) => {
  button.addEventListener("click", () => copyText(button.dataset.copy));
});

copyCommand.addEventListener("click", () => copyText(runCommand(active)));

renderTopic(active);
