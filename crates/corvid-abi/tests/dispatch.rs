mod common;

use common::emit_descriptor;
use corvid_abi::AbiDispatch;

#[test]
fn progressive_dispatch_emits_stages_with_thresholds() {
    let abi = emit_descriptor(
        r#"
model basic:
    capability: basic
model standard:
    capability: standard
model expert:
    capability: expert

prompt classify(ticket: String) -> String:
    progressive:
        basic below 0.80
        standard below 0.90
        expert
    "classify {ticket}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    return classify(ticket) == "refund"
"#,
    );
    let prompt = abi
        .prompts
        .iter()
        .find(|prompt| prompt.name == "classify")
        .unwrap();
    match prompt.dispatch.as_ref().unwrap() {
        AbiDispatch::Progressive { stages } => {
            assert_eq!(stages.len(), 3);
            assert_eq!(stages[0].model_requires, "basic");
            assert_eq!(stages[0].escalate_below_confidence, Some(0.80));
        }
        other => panic!("expected progressive dispatch, got {other:?}"),
    }
}

#[test]
fn rollout_dispatch_emits_variant_and_baseline() {
    let abi = emit_descriptor(
        r#"
model v1:
    capability: basic
model v2:
    capability: standard

prompt classify(ticket: String) -> String:
    rollout 10% v2, else v1
    "classify {ticket}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    return classify(ticket) == "refund"
"#,
    );
    let prompt = abi
        .prompts
        .iter()
        .find(|prompt| prompt.name == "classify")
        .unwrap();
    match prompt.dispatch.as_ref().unwrap() {
        AbiDispatch::Rollout {
            variant,
            baseline,
            variant_percent,
        } => {
            assert_eq!(variant, "v2");
            assert_eq!(baseline, "v1");
            assert_eq!(*variant_percent, 10.0);
        }
        other => panic!("expected rollout dispatch, got {other:?}"),
    }
}

#[test]
fn ensemble_dispatch_emits_model_list_and_vote_strategy() {
    let abi = emit_descriptor(
        r#"
model a:
    capability: basic
model b:
    capability: standard
model c:
    capability: expert

prompt classify(ticket: String) -> String:
    ensemble [a, b, c] vote majority
    "classify {ticket}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    return classify(ticket) == "refund"
"#,
    );
    let prompt = abi
        .prompts
        .iter()
        .find(|prompt| prompt.name == "classify")
        .unwrap();
    match prompt.dispatch.as_ref().unwrap() {
        AbiDispatch::Ensemble {
            models,
            vote_strategy,
        } => {
            assert_eq!(
                models,
                &vec!["a".to_string(), "b".to_string(), "c".to_string()]
            );
            assert_eq!(vote_strategy, "majority");
        }
        other => panic!("expected ensemble dispatch, got {other:?}"),
    }
}

#[test]
fn adversarial_dispatch_emits_propose_challenge_adjudicate_stages() {
    let abi = emit_descriptor(
        r#"
prompt propose_answer(ticket: String) -> String:
    "p {ticket}"
prompt critique(proposed: String) -> String:
    "c {proposed}"
type Verdict:
    contradiction: Bool
    result: String
prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    "a {proposed} {flaws}"

prompt classify(ticket: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    "classify {ticket}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    verdict = classify(ticket)
    return verdict.contradiction
"#,
    );
    let prompt = abi
        .prompts
        .iter()
        .find(|prompt| prompt.name == "classify")
        .unwrap();
    match prompt.dispatch.as_ref().unwrap() {
        AbiDispatch::Adversarial {
            propose,
            challenge,
            adjudicate,
        } => {
            assert_eq!(propose, "propose_answer");
            assert_eq!(challenge, "critique");
            assert_eq!(adjudicate, "adjudicate_fn");
        }
        other => panic!("expected adversarial dispatch, got {other:?}"),
    }
}

#[test]
fn direct_dispatch_emits_null_dispatch() {
    let abi = emit_descriptor(
        r#"
prompt classify(ticket: String) -> String:
    "classify {ticket}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    return classify(ticket) == "refund"
"#,
    );
    let prompt = abi
        .prompts
        .iter()
        .find(|prompt| prompt.name == "classify")
        .unwrap();
    assert!(prompt.dispatch.is_none());
}
