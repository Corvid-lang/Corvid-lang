use anyhow::{anyhow, Result};

pub struct TourTopic {
    pub name: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub pitch: &'static str,
    pub spec: &'static str,
    pub roadmap: &'static str,
    pub test: &'static str,
    pub non_scope: &'static str,
    pub source: &'static str,
}

pub const TOPICS: &[TourTopic] = &[
    TourTopic {
        name: "approve-gates",
        title: "Approve Before Dangerous",
        category: "Safety at compile time",
        pitch: "Dangerous actions are not library conventions. The compiler requires an explicit approval boundary before irreversible tools run.",
        spec: "docs/effects-spec/03-typing-rules.md",
        roadmap: "Phase 20 safety wave",
        test: "crates/corvid-types/src/lib.rs approval checker tests",
        non_scope: "Does not decide whether a human should approve; it proves the approval boundary exists.",
        source: r#"type Receipt:
    id: String

tool issue_refund(id: String) -> Receipt dangerous

agent refund(id: String) -> Receipt:
    approve IssueRefund(id)
    return issue_refund(id)
"#,
    },
    TourTopic {
        name: "dimensional-effects",
        title: "Dimensional Effects",
        category: "Safety at compile time",
        pitch: "Effects are not flat tags. Cost, trust, reversibility, data, latency, confidence, and user dimensions compose with their own algebra.",
        spec: "docs/effects-spec/02-composition-algebra.md",
        roadmap: "Phase 20a and Phase 20g",
        test: "crates/corvid-types/src/effects.rs composition tests",
        non_scope: "Does not make external providers honest; it proves the declared Corvid contract.",
        source: r#"effect llm_call:
    cost: $0.05
    trust: autonomous

prompt summarize(text: String) -> String uses llm_call:
    "Summarize {text}"

@budget($0.10)
@trust(autonomous)
agent summarize_twice(text: String) -> String:
    first = summarize(text)
    return summarize(first)
"#,
    },
    TourTopic {
        name: "grounded-values",
        title: "Grounded<T> Provenance",
        category: "Safety at compile time",
        pitch: "A grounded return must flow from a retrieval source. At runtime the value carries the provenance chain that made it grounded.",
        spec: "docs/effects-spec/05-grounding.md",
        roadmap: "Phase 20b",
        test: "crates/corvid-types/src/effects/grounded.rs tests",
        non_scope: "Does not prove the retrieved document is true; it proves the answer is sourced.",
        source: r#"effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

prompt summarize(doc: Grounded<String>) -> Grounded<String>:
    "Summarize {doc}"

agent research(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    return summarize(doc)
"#,
    },
    TourTopic {
        name: "strict-citations",
        title: "Strict Citation Contracts",
        category: "Safety at compile time",
        pitch: "A prompt can name the grounded context it must cite. The compiler proves the cited parameter is grounded; runtime checks the response.",
        spec: "docs/effects-spec/05-grounding.md",
        roadmap: "Phase 20b cites ctx strictly",
        test: "crates/corvid-vm/src/tests/dispatch.rs citation tests",
        non_scope: "Strict citation checks text evidence; they do not judge source truth.",
        source: r#"effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

prompt answer(ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    "Answer from {ctx}"

agent cited(id: String) -> Grounded<String>:
    return answer(fetch_doc(id))
"#,
    },
    TourTopic {
        name: "cost-budgets",
        title: "Compile-Time Budgets",
        category: "Safety at compile time",
        pitch: "Budget annotations are static constraints over the composed cost tree, not billing dashboards after the model call has run.",
        spec: "docs/effects-spec/07-cost-budgets.md",
        roadmap: "Phase 20d",
        test: "crates/corvid-types/src/effects/cost.rs tests",
        non_scope: "Static budgets use declared costs; provider invoices still need operational reconciliation.",
        source: r#"effect cheap_call:
    cost: $0.05

prompt classify(text: String) -> String uses cheap_call:
    "Classify {text}"

@budget($0.10)
agent bounded(text: String) -> String:
    first = classify(text)
    return classify(first)
"#,
    },
    TourTopic {
        name: "confidence-gates",
        title: "Confidence As A Dimension",
        category: "Safety at compile time",
        pitch: "Confidence composes by weakest link. Agents can require a floor, and trust gates can escalate when confidence drops.",
        spec: "docs/effects-spec/06-confidence-gates.md",
        roadmap: "Phase 20e",
        test: "crates/corvid-types/src/tests.rs min_confidence tests",
        non_scope: "Confidence is only meaningful when model adapters provide calibrated signals.",
        source: r#"effect llm_decision:
    confidence: 0.95

tool search(query: String) -> String uses llm_decision

@min_confidence(0.90)
agent bot(query: String) -> String:
    return search(query)
"#,
    },
    TourTopic {
        name: "eval-traces",
        title: "Trace-Aware Evals",
        category: "AI-native ergonomics",
        pitch: "Evals can assert process, not just output. Corvid checks whether the agent called, approved, ordered, and spent as intended.",
        spec: "docs/effects-spec/12-verification.md",
        roadmap: "Phase 20c",
        test: "crates/corvid-types/src/lib.rs eval assertion tests",
        non_scope: "This is language and checker support; the full eval runner is later workflow tooling.",
        source: r#"agent always_refund() -> Bool:
    return true

eval refund_accuracy:
    result = always_refund()
    assert result == true
"#,
    },
    TourTopic {
        name: "language-keywords",
        title: "AI-Native Keywords",
        category: "AI-native ergonomics",
        pitch: "Agents, tools, prompts, effects, approvals, models, evals, and streams are syntax the compiler can reason about.",
        spec: "docs/effects-spec/01-dimensional-syntax.md",
        roadmap: "Phase 20 language surface",
        test: "crates/corvid-syntax/src/parser/tests.rs",
        non_scope: "Keywords do not replace ordinary general-purpose code; they make AI boundaries visible.",
        source: r#"model local:
    capability: basic

prompt say(name: String) -> String:
    requires: basic
    "Hello {name}"

agent hello(name: String) -> String:
    return say(name)
"#,
    },
    TourTopic {
        name: "streaming-effects",
        title: "Streaming Effects",
        category: "Streaming",
        pitch: "Streams are typed values that carry effects mid-flight. Budgets, confidence, provenance, and backpressure are not after-the-fact logs.",
        spec: "docs/effects-spec/08-streaming.md",
        roadmap: "Phase 20f",
        test: "crates/corvid-vm/src/tests/stream.rs",
        non_scope: "Provider-native continuation depends on provider APIs; local typed fallback tokens are the shipped boundary.",
        source: r#"agent count() -> Stream<Int>:
    yield 1
    yield 2
"#,
    },
    TourTopic {
        name: "partial-streams",
        title: "Progressive Structured Streams",
        category: "Streaming",
        pitch: "Partial<T> lets the program read complete fields as they arrive while the rest of a structured stream is still forming.",
        spec: "docs/effects-spec/08-streaming.md",
        roadmap: "Phase 20f Stream<Partial<T>>",
        test: "crates/corvid-types/src/tests.rs partial stream tests",
        non_scope: "Native codegen support for every partial-stream path is still bounded by backend parity work.",
        source: r#"type Plan:
    title: String
    body: String

agent read(snapshot: Partial<Plan>) -> Option<String>:
    return snapshot.title
"#,
    },
    TourTopic {
        name: "stream-resume",
        title: "Typed Stream Resumption",
        category: "Streaming",
        pitch: "A ResumeToken<T> captures the typed stream element contract, so continuation cannot resume the wrong prompt shape.",
        spec: "docs/effects-spec/08-streaming.md",
        roadmap: "Phase 20f resumption tokens",
        test: "crates/corvid-vm/src/tests/stream.rs resume tests",
        non_scope: "Provider-native session continuation waits on provider APIs; local fallback is shipped.",
        source: r#"prompt draft(topic: String) -> Stream<String>:
    "Draft {topic}"

agent capture(topic: String) -> ResumeToken<String>:
    stream = draft(topic)
    return resume_token(stream)

agent continue_it(token: ResumeToken<String>) -> Stream<String>:
    return resume(draft, token)
"#,
    },
    TourTopic {
        name: "stream-fanout",
        title: "Declarative Fan-Out / Fan-In",
        category: "Streaming",
        pitch: "Streams can split by structured fields and merge back with deterministic ordering, preserving typed stream effects.",
        spec: "docs/effects-spec/08-streaming.md",
        roadmap: "Phase 20f fan-out/fan-in",
        test: "crates/corvid-types/src/tests.rs stream_split_merge_ordered_by_typechecks",
        non_scope: "Field-keyed split is shipped; first-class lambda extractors wait for function values.",
        source: r#"type Event:
    kind: String
    body: String

agent source() -> Stream<Event>:
    yield Event("b", "two")
    yield Event("a", "one")

agent fanout() -> Stream<Event>:
    groups = source().split_by("kind")
    return merge(groups).ordered_by("fair_round_robin")
"#,
    },
    TourTopic {
        name: "model-routing",
        title: "Typed Model Routing",
        category: "Adaptive routing",
        pitch: "Models are typed declarations with capability and policy dimensions. Prompt dispatch is checked against those model contracts.",
        spec: "docs/effects-spec/13-model-substrate-shipped.md",
        roadmap: "Phase 20h",
        test: "crates/corvid-vm/src/tests/dispatch.rs",
        non_scope: "Does not benchmark model quality automatically; routing reports use recorded eval history.",
        source: r#"model fast:
    capability: basic

model deep:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == "hard" -> deep
        _ -> fast
    "Answer {q}"
"#,
    },
    TourTopic {
        name: "progressive-routing",
        title: "Progressive Refinement",
        category: "Adaptive routing",
        pitch: "A prompt can try cheap models first and escalate only when confidence falls below a typed threshold.",
        spec: "docs/effects-spec/13-model-substrate-shipped.md#135-progressive-refinement-slice-e",
        roadmap: "Phase 20h slice E",
        test: "crates/corvid-vm/src/tests/dispatch.rs progressive tests",
        non_scope: "Thresholds are only meaningful when adapters report calibrated confidence.",
        source: r#"model cheap:
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
    "Classify {q}"
"#,
    },
    TourTopic {
        name: "ensemble-voting",
        title: "Ensemble Voting",
        category: "Adaptive routing",
        pitch: "One prompt can dispatch to several models concurrently and fold the answers through a typed voting strategy.",
        spec: "docs/effects-spec/13-model-substrate-shipped.md#137-ensemble-voting-slice-f",
        roadmap: "Phase 20h slice F",
        test: "crates/corvid-vm/src/tests/dispatch.rs ensemble tests",
        non_scope: "Majority voting is shipped; arbitrary custom vote functions are future language work.",
        source: r#"model opus:
    capability: expert

model sonnet:
    capability: expert

model haiku:
    capability: standard

prompt classify(q: String) -> String:
    ensemble [opus, sonnet, haiku] vote majority
    "Classify {q}"
"#,
    },
    TourTopic {
        name: "privacy-routing",
        title: "Jurisdiction And Privacy Dimensions",
        category: "Adaptive routing",
        pitch: "Model selection can carry regulatory dimensions such as jurisdiction, compliance, and privacy tier as typed model facts.",
        spec: "docs/effects-spec/13-model-substrate-shipped.md#134-regulatory--compliance--privacy-dimensions-slice-d",
        roadmap: "Phase 20h slice D",
        test: "crates/corvid-types/src/effects.rs dimension law tests",
        non_scope: "The compiler enforces declared routing facts; legal compliance still requires operational controls.",
        source: r#"model eu_private:
    jurisdiction: eu_hosted
    compliance: gdpr
    privacy_tier: strict
    capability: expert

model us_fast:
    jurisdiction: us_hosted
    privacy_tier: standard
    capability: basic
"#,
    },
    TourTopic {
        name: "replay-receipts",
        title: "Replay And Receipts",
        category: "Verification",
        pitch: "Executions become evidence. Traces, replay, trace-diff, and signed receipts turn behavior changes into reviewable artifacts.",
        spec: "docs/effects-spec/14-replay.md",
        roadmap: "Phase 21 and Phase 22",
        test: "crates/corvid-cli/tests/bundle_verify.rs",
        non_scope: "A receipt is cryptographic evidence of observed behavior, not a formal proof of all possible runs.",
        source: r#"@deterministic
@replayable
agent classify(text: String) -> String:
    return text
"#,
    },
    TourTopic {
        name: "effect-registry",
        title: "Proof-Carrying Dimension Registry",
        category: "Verification",
        pitch: "Corvid can distribute pieces of the effect algebra as signed artifacts with law checks, proofs, and regression programs.",
        spec: "docs/effects-spec/dimension-artifacts.md",
        roadmap: "Phase 20g invention #9",
        test: "crates/corvid-driver/src/dimension_registry.rs tests",
        non_scope: "The registry distributes declarations, not executable code or unverified trust.",
        source: r#"effect local_policy:
    data: pii
    reversible: true

tool read_profile(id: String) -> String uses local_policy

agent profile(id: String) -> String:
    return read_profile(id)
"#,
    },
    TourTopic {
        name: "adversarial-tests",
        title: "Adversarial Bypass Testing",
        category: "Verification",
        pitch: "The compiler ships with a bypass-attempt taxonomy so AI can attack Corvid's own effect system in CI.",
        spec: "docs/effects-spec/adversarial-taxonomy.md",
        roadmap: "Phase 20g adversarial generator",
        test: "crates/corvid-driver/src/adversarial.rs tests",
        non_scope: "Live LLM generation expands the corpus; deterministic seeds remain the safety gate.",
        source: r#"effect transfer_money:
    trust: human_required
    reversible: false

tool refund(id: String) -> String dangerous uses transfer_money

@trust(human_required)
agent safe_refund(id: String) -> String:
    approve Refund(id)
    return refund(id)
"#,
    },
];

pub fn cmd_tour(list: bool, topic: Option<&str>) -> Result<u8> {
    if list || topic.is_none() {
        print!("{}", render_tour_list());
        return Ok(0);
    }
    let topic_name = topic.unwrap();
    let topic = find_topic(topic_name)
        .ok_or_else(|| anyhow!("unknown tour topic `{topic_name}`; run `corvid tour --list`"))?;
    print!("{}", render_topic_card(topic));
    corvid_repl::Repl::run_tour_stdio(topic.title, topic.source)?;
    Ok(0)
}

pub fn find_topic(name: &str) -> Option<&'static TourTopic> {
    TOPICS.iter().find(|topic| topic.name == name)
}

pub fn render_tour_list() -> String {
    let mut out = String::new();
    out.push_str("Corvid invention tour\n\n");
    for topic in TOPICS {
        out.push_str(&format!(
            "  {:<22} {:<24} {}\n",
            topic.name, topic.category, topic.title
        ));
    }
    out.push_str("\nRun `corvid tour --topic <name>` to load a demo into the REPL.\n");
    out
}

pub fn render_topic_card(topic: &TourTopic) -> String {
    format!(
        "Topic: {title}\nCategory: {category}\n\n{pitch}\n\nSpec: {spec}\nRoadmap: {roadmap}\nTest: {test}\nNon-scope: {non_scope}\n\n",
        title = topic.title,
        category = topic.category,
        pitch = topic.pitch,
        spec = topic.spec,
        roadmap = topic.roadmap,
        test = topic.test,
        non_scope = topic.non_scope,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_mentions_every_topic_name() {
        let list = render_tour_list();
        for topic in TOPICS {
            assert!(list.contains(topic.name), "missing {}", topic.name);
        }
    }

    #[test]
    fn all_tour_sources_compile() {
        for topic in TOPICS {
            let compiled = corvid_driver::compile(topic.source);
            assert!(
                compiled.ok(),
                "tour topic `{}` failed to compile: {:?}",
                topic.name,
                compiled.diagnostics
            );
        }
    }
}
