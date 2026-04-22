use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corvid_abi::{
    AbiAgent, AbiApprovalContract, AbiApprovalLabel, AbiAttributes, AbiBudget, AbiEffects,
    AbiField, AbiGroundedType, AbiParam, AbiProvenanceContract, AbiSourceSpan, AbiTypeDecl,
    CorvidAbi, ScalarTypeName, TypeDescription,
};
use corvid_bind::{generate_bindings_from_descriptor_path, BindLanguage};
use tempfile::TempDir;

fn sample_descriptor() -> CorvidAbi {
    CorvidAbi {
        corvid_abi_version: 1,
        compiler_version: "0.0.1-test".to_string(),
        source_path: "examples/cdylib_catalog_demo/src/classify.cor".to_string(),
        generated_at: "2026-04-22T00:00:00Z".to_string(),
        agents: vec![
            AbiAgent {
                name: "classify".to_string(),
                symbol: "classify".to_string(),
                source_span: AbiSourceSpan { start: 0, end: 1 },
                source_line: 8,
                params: vec![AbiParam {
                    name: "text".to_string(),
                    ty: TypeDescription::Scalar {
                        scalar: ScalarTypeName::String,
                    },
                }],
                return_type: TypeDescription::Scalar {
                    scalar: ScalarTypeName::String,
                },
                effects: AbiEffects {
                    trust_tier: Some("autonomous".to_string()),
                    ..AbiEffects::default()
                },
                attributes: AbiAttributes {
                    replayable: true,
                    deterministic: true,
                    dangerous: false,
                    pub_extern_c: true,
                },
                budget: Some(AbiBudget { usd_per_call: 0.01 }),
                required_capability: None,
                dispatch: None,
                approval_contract: AbiApprovalContract {
                    required: false,
                    labels: Vec::new(),
                },
                provenance: AbiProvenanceContract {
                    returns_grounded: false,
                    grounded_param_deps: Vec::new(),
                },
            },
            AbiAgent {
                name: "issue_tag".to_string(),
                symbol: "issue_tag".to_string(),
                source_span: AbiSourceSpan { start: 2, end: 3 },
                source_line: 12,
                params: vec![AbiParam {
                    name: "tag".to_string(),
                    ty: TypeDescription::Scalar {
                        scalar: ScalarTypeName::String,
                    },
                }],
                return_type: TypeDescription::Scalar {
                    scalar: ScalarTypeName::String,
                },
                effects: AbiEffects {
                    trust_tier: Some("human_required".to_string()),
                    ..AbiEffects::default()
                },
                attributes: AbiAttributes {
                    replayable: true,
                    deterministic: true,
                    dangerous: true,
                    pub_extern_c: true,
                },
                budget: Some(AbiBudget { usd_per_call: 0.02 }),
                required_capability: None,
                dispatch: None,
                approval_contract: AbiApprovalContract {
                    required: true,
                    labels: vec![AbiApprovalLabel {
                        label: "EchoString".to_string(),
                        args: vec![AbiParam {
                            name: "value".to_string(),
                            ty: TypeDescription::Scalar {
                                scalar: ScalarTypeName::String,
                            },
                        }],
                        cost_at_site: Some(0.02),
                        reversibility: Some("reversible".to_string()),
                        required_tier: Some("human_required".to_string()),
                    }],
                },
                provenance: AbiProvenanceContract {
                    returns_grounded: false,
                    grounded_param_deps: Vec::new(),
                },
            },
            AbiAgent {
                name: "grounded_tag".to_string(),
                symbol: "grounded_tag".to_string(),
                source_span: AbiSourceSpan { start: 4, end: 5 },
                source_line: 17,
                params: vec![AbiParam {
                    name: "tag".to_string(),
                    ty: TypeDescription::Scalar {
                        scalar: ScalarTypeName::String,
                    },
                }],
                return_type: TypeDescription::Grounded {
                    grounded: AbiGroundedType {
                        inner: Box::new(TypeDescription::Scalar {
                            scalar: ScalarTypeName::String,
                        }),
                    },
                },
                effects: AbiEffects {
                    trust_tier: Some("autonomous".to_string()),
                    ..AbiEffects::default()
                },
                attributes: AbiAttributes {
                    replayable: true,
                    deterministic: true,
                    dangerous: false,
                    pub_extern_c: true,
                },
                budget: Some(AbiBudget { usd_per_call: 0.01 }),
                required_capability: None,
                dispatch: None,
                approval_contract: AbiApprovalContract {
                    required: false,
                    labels: Vec::new(),
                },
                provenance: AbiProvenanceContract {
                    returns_grounded: true,
                    grounded_param_deps: Vec::new(),
                },
            },
        ],
        prompts: Vec::new(),
        tools: Vec::new(),
        types: vec![AbiTypeDecl {
            name: "Decision".to_string(),
            kind: "struct".to_string(),
            fields: vec![
                AbiField {
                    name: "amount".to_string(),
                    r#type: TypeDescription::Scalar {
                        scalar: ScalarTypeName::Float,
                    },
                },
                AbiField {
                    name: "approved".to_string(),
                    r#type: TypeDescription::Scalar {
                        scalar: ScalarTypeName::Bool,
                    },
                },
            ],
        }],
        approval_sites: Vec::new(),
        extra: BTreeMap::new(),
    }
}

fn dangerous_grounded_descriptor() -> CorvidAbi {
    let mut abi = sample_descriptor();
    abi.agents = vec![AbiAgent {
        name: "dangerous_grounded".to_string(),
        symbol: "dangerous_grounded".to_string(),
        source_span: AbiSourceSpan { start: 0, end: 1 },
        source_line: 1,
        params: vec![AbiParam {
            name: "tag".to_string(),
            ty: TypeDescription::Scalar {
                scalar: ScalarTypeName::String,
            },
        }],
        return_type: TypeDescription::Grounded {
            grounded: AbiGroundedType {
                inner: Box::new(TypeDescription::Scalar {
                    scalar: ScalarTypeName::String,
                }),
            },
        },
        effects: AbiEffects::default(),
        attributes: AbiAttributes {
            replayable: true,
            deterministic: true,
            dangerous: true,
            pub_extern_c: true,
        },
        budget: Some(AbiBudget { usd_per_call: 0.5 }),
        required_capability: None,
        dispatch: None,
        approval_contract: AbiApprovalContract {
            required: true,
            labels: vec![AbiApprovalLabel {
                label: "EchoString".to_string(),
                args: Vec::new(),
                cost_at_site: None,
                reversibility: None,
                required_tier: Some("human_required".to_string()),
            }],
        },
        provenance: AbiProvenanceContract {
            returns_grounded: true,
            grounded_param_deps: Vec::new(),
        },
    }];
    abi
}

fn write_descriptor(temp: &TempDir, abi: &CorvidAbi, stem: &str) -> PathBuf {
    let path = temp.path().join(format!("{stem}.corvid-abi.json"));
    std::fs::write(&path, serde_json::to_string_pretty(abi).expect("descriptor json"))
        .expect("write descriptor");
    path
}

fn collect_files(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .expect("relative path")
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path).expect("read generated file");
            files.insert(rel, text);
        }
    }
    files
}

#[test]
fn rust_generation_is_reproducible_and_keeps_semantic_surface() {
    let temp = tempfile::tempdir().expect("tempdir");
    let descriptor = write_descriptor(&temp, &sample_descriptor(), "classify");
    let first = temp.path().join("rust_first");
    let second = temp.path().join("rust_second");

    generate_bindings_from_descriptor_path(BindLanguage::Rust, &descriptor, &first)
        .expect("generate rust bindings");
    generate_bindings_from_descriptor_path(BindLanguage::Rust, &descriptor, &second)
        .expect("generate rust bindings again");

    let first_files = collect_files(&first);
    let second_files = collect_files(&second);
    assert_eq!(first_files, second_files, "Rust binding output drifted");

    insta::assert_snapshot!(
        "rust_common_rs",
        first_files.get("src/common.rs").expect("common.rs")
    );
    insta::assert_snapshot!(
        "rust_issue_tag_rs",
        first_files.get("src/issue_tag.rs").expect("issue_tag.rs")
    );
    insta::assert_snapshot!(
        "rust_catalog_rs",
        first_files.get("src/catalog.rs").expect("catalog.rs")
    );
}

#[test]
fn python_generation_is_reproducible_and_keeps_semantic_surface() {
    let temp = tempfile::tempdir().expect("tempdir");
    let descriptor = write_descriptor(&temp, &sample_descriptor(), "classify");
    let first = temp.path().join("python_first");
    let second = temp.path().join("python_second");

    generate_bindings_from_descriptor_path(BindLanguage::Python, &descriptor, &first)
        .expect("generate python bindings");
    generate_bindings_from_descriptor_path(BindLanguage::Python, &descriptor, &second)
        .expect("generate python bindings again");

    let first_files = collect_files(&first);
    let second_files = collect_files(&second);
    assert_eq!(first_files, second_files, "Python binding output drifted");

    insta::assert_snapshot!(
        "python_common_py",
        first_files.get("classify/common.py").expect("common.py")
    );
    insta::assert_snapshot!(
        "python_issue_tag_py",
        first_files
            .get("classify/issue_tag.py")
            .expect("issue_tag.py")
    );
    insta::assert_snapshot!(
        "python_catalog_py",
        first_files.get("classify/catalog.py").expect("catalog.py")
    );
}

#[test]
fn dangerous_grounded_exports_fail_validation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let descriptor = write_descriptor(&temp, &dangerous_grounded_descriptor(), "dangerous_grounded");
    let out = temp.path().join("out");
    let err = generate_bindings_from_descriptor_path(BindLanguage::Rust, &descriptor, &out)
        .expect_err("dangerous grounded bindings should fail");
    let message = format!("{err:#}");
    assert!(
        message.contains("dangerous grounded exports"),
        "unexpected error: {message}"
    );
}
