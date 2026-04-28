use crate::schema::{
    AbiGroundedType, AbiListType, AbiOptionType, AbiPartialType, AbiResultType, AbiResumeTokenType,
    AbiWeakType, ScalarTypeName, TypeDescription,
};
use corvid_ast::WeakEffect;
use corvid_resolve::{DefId, Resolved};
use corvid_types::Type;

pub fn emit_type_description(ty: &Type, resolved: &Resolved) -> TypeDescription {
    match ty {
        Type::Int => TypeDescription::Scalar {
            scalar: ScalarTypeName::Int,
        },
        Type::Float => TypeDescription::Scalar {
            scalar: ScalarTypeName::Float,
        },
        Type::String => TypeDescription::Scalar {
            scalar: ScalarTypeName::String,
        },
        Type::Bool => TypeDescription::Scalar {
            scalar: ScalarTypeName::Bool,
        },
        Type::Nothing => TypeDescription::Scalar {
            scalar: ScalarTypeName::Nothing,
        },
        Type::TraceId => TypeDescription::Scalar {
            scalar: ScalarTypeName::TraceId,
        },
        Type::Struct(def_id) => TypeDescription::Struct {
            name: lookup_name(resolved, *def_id),
        },
        Type::ImportedStruct(imported) => TypeDescription::Struct {
            name: imported.name.clone(),
        },
        Type::List(inner) | Type::Stream(inner) => TypeDescription::List {
            list: AbiListType {
                element: Box::new(emit_type_description(inner, resolved)),
            },
        },
        Type::Result(ok, err) => TypeDescription::Result {
            result: AbiResultType {
                ok: Box::new(emit_type_description(ok, resolved)),
                err: Box::new(emit_type_description(err, resolved)),
            },
        },
        Type::Option(inner) => TypeDescription::Option {
            option: AbiOptionType {
                inner: Box::new(emit_type_description(inner, resolved)),
            },
        },
        Type::Grounded(inner) => TypeDescription::Grounded {
            grounded: AbiGroundedType {
                inner: Box::new(emit_type_description(inner, resolved)),
            },
        },
        Type::Partial(inner) => TypeDescription::Partial {
            partial: AbiPartialType {
                inner: Box::new(emit_type_description(inner, resolved)),
            },
        },
        Type::ResumeToken(inner) => TypeDescription::ResumeToken {
            resume_token: AbiResumeTokenType {
                inner: Box::new(emit_type_description(inner, resolved)),
            },
        },
        Type::Weak(inner, effects) => TypeDescription::Weak {
            weak: AbiWeakType {
                inner: Box::new(emit_type_description(inner, resolved)),
                effects: effects
                    .effects()
                    .into_iter()
                    .map(|effect| match effect {
                        WeakEffect::ToolCall => "tool_call".to_string(),
                        WeakEffect::Llm => "llm_call".to_string(),
                        WeakEffect::Approve => "approve".to_string(),
                        WeakEffect::Human => "human".to_string(),
                    })
                    .collect(),
            },
        },
        Type::Function { .. } | Type::Unknown => TypeDescription::Scalar {
            scalar: ScalarTypeName::String,
        },
    }
}

fn lookup_name(resolved: &Resolved, def_id: DefId) -> String {
    resolved.symbols.get(def_id).name.clone()
}
