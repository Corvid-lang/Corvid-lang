    use super::*;
    use corvid_ast::File;
    use corvid_guarantees::lookup as lookup_guarantee;

    fn parse_source(source: &str) -> File {
        let tokens = lex(source).expect("lex");
        let (file, errors) = parse_file(&tokens);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        file
    }

    fn descriptor_with_claim_ids(ids: &[&str]) -> String {
        let claims = ids
            .iter()
            .map(|id| {
                let guarantee = lookup_guarantee(id).expect("registered guarantee");
                serde_json::json!({
                    "id": guarantee.id,
                    "kind": guarantee.kind.slug(),
                    "class": guarantee.class.slug(),
                    "phase": guarantee.phase.slug(),
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "corvid_abi_version": corvid_abi::CORVID_ABI_VERSION,
            "compiler_version": "test",
            "source_path": "test.cor",
            "generated_at": "1970-01-01T00:00:00Z",
            "agents": [],
            "prompts": [],
            "tools": [],
            "types": [],
            "stores": [],
            "approval_sites": [],
            "claim_guarantees": claims,
        })
        .to_string()
    }

    #[test]
    fn signed_claim_coverage_accepts_registered_contracts() {
        let file = parse_source(
            r#"
effect transfer:
    cost: $0.01

tool issue_refund(id: String) -> String dangerous uses transfer

@budget($0.50)
@replayable
pub extern "c"
agent refund(id: String) -> String uses transfer:
    approve issue_refund(id)
    return issue_refund(id)
"#,
        );
        let descriptor =
            descriptor_with_claim_ids(corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS);
        validate_signed_claim_coverage(&file, &descriptor).expect("coverage accepted");
    }

    #[test]
    fn signed_claim_coverage_rejects_missing_declared_contract_id() {
        let file = parse_source(
            r#"
tool issue_refund(id: String) -> String dangerous

pub extern "c"
agent refund(id: String) -> String:
    approve issue_refund(id)
    return issue_refund(id)
"#,
        );
        let ids = corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS
            .iter()
            .copied()
            .filter(|id| *id != "approval.dangerous_call_requires_token")
            .collect::<Vec<_>>();
        let descriptor = descriptor_with_claim_ids(&ids);
        let err = validate_signed_claim_coverage(&file, &descriptor)
            .expect_err("missing approval claim must reject signing");
        assert!(
            err.to_string()
                .contains("approval.dangerous_call_requires_token"),
            "{err:#}"
        );
    }

    #[test]
    fn signed_claim_coverage_rejects_out_of_scope_contract_id() {
        let file = parse_source(
            r#"
pub extern "c"
agent answer(x: Int) -> Int:
    return x
"#,
        );
        let mut ids = corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS.to_vec();
        ids.push("platform.signing_key_compromise");
        let descriptor = descriptor_with_claim_ids(&ids);
        let err = validate_signed_claim_coverage(&file, &descriptor)
            .expect_err("out-of-scope claim must reject signing");
        assert!(
            err.to_string().contains("out_of_scope"),
            "{err:#}"
        );
    }

    /// Slice 35-N positive: a `Decl::Schedule` raises a require for
    /// `jobs.cron_schedule_durable` and the gate accepts when the
    /// descriptor includes that id.
    #[test]
    fn signed_claim_coverage_walks_schedule_decl() {
        let file = parse_source(
            r#"
effect send_email:
    cost: $0.05

agent daily_brief(user_id: String) -> String uses send_email:
    return user_id

schedule "0 8 * * *" zone "America/New_York" -> daily_brief("u1") uses send_email
"#,
        );
        let descriptor =
            descriptor_with_claim_ids(corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS);
        validate_signed_claim_coverage(&file, &descriptor)
            .expect("schedule decl must be accepted when jobs.cron_schedule_durable is in claims");
    }

    /// Slice 35-N adversarial: a `Decl::Schedule` without the
    /// `jobs.cron_schedule_durable` claim id in the descriptor must
    /// be refused: a signed cdylib that ships a cron trigger must
    /// acknowledge that contract.
    #[test]
    fn signed_claim_coverage_rejects_schedule_without_jobs_coverage() {
        let file = parse_source(
            r#"
effect send_email:
    cost: $0.05

agent daily_brief(user_id: String) -> String uses send_email:
    return user_id

schedule "0 8 * * *" zone "America/New_York" -> daily_brief("u1") uses send_email
"#,
        );
        let ids = corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS
            .iter()
            .copied()
            .filter(|id| *id != "jobs.cron_schedule_durable")
            .collect::<Vec<_>>();
        let descriptor = descriptor_with_claim_ids(&ids);
        let err = validate_signed_claim_coverage(&file, &descriptor)
            .expect_err("schedule without cron_schedule_durable must reject signing");
        assert!(
            err.to_string().contains("jobs.cron_schedule_durable"),
            "{err:#}"
        );
    }
