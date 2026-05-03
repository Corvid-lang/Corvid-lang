use corvid_ast::{
    AgentDecl, Backoff, BackpressurePolicy, BinaryOp, Block, Decl, DimensionValue, Effect,
    EffectConstraint, EvalAssert, Expr, ExtendMethod, ExtendMethodKind, File, FixtureDecl,
    ImportSource, Literal, MockDecl, Param, PromptDecl, Stmt, ToolDecl, TypeRef, UnaryOp,
    Visibility,
};
use std::collections::BTreeSet;

use crate::{DataCategory, DivergenceClass, DivergenceReport, LatencyLevel, TrustLevel};

pub fn render_file(file: &File) -> String {
    let mut out = String::new();
    for (index, decl) in file.decls.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        render_decl(decl, 0, &mut out);
    }
    out.push('\n');
    out
}

fn render_decl(decl: &Decl, indent: usize, out: &mut String) {
    match decl {
        Decl::Import(import) => {
            push_indent(indent, out);
            out.push_str("import ");
            out.push_str(match import.source {
                ImportSource::Python => "python ",
                ImportSource::Corvid | ImportSource::RemoteCorvid | ImportSource::PackageCorvid => {
                    ""
                }
            });
            out.push_str(&render_string_literal(&import.module));
            if let Some(hash) = &import.content_hash {
                out.push_str(" hash:");
                out.push_str(&hash.algorithm);
                out.push(':');
                out.push_str(&hash.hex);
            }
            if let Some(alias) = &import.alias {
                out.push_str(" as ");
                out.push_str(&alias.name);
            }
            if !import.effect_row.effects.is_empty() {
                out.push_str(" effects: ");
                for (index, effect) in import.effect_row.effects.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&effect.name.name);
                }
            }
        }
        Decl::Type(ty) => {
            push_indent(indent, out);
            out.push_str("type ");
            out.push_str(&ty.name.name);
            out.push_str(":\n");
            for (index, field) in ty.fields.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&field.name.name);
                out.push_str(": ");
                out.push_str(&render_type_ref(&field.ty));
            }
        }
        Decl::Store(store) => {
            push_indent(indent, out);
            out.push_str(store.kind.as_str());
            out.push(' ');
            out.push_str(&store.name.name);
            out.push_str(":\n");
            for (index, field) in store.fields.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&field.name.name);
                out.push_str(": ");
                out.push_str(&render_type_ref(&field.ty));
            }
            for (index, policy) in store.policies.iter().enumerate() {
                if !store.fields.is_empty() || index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str("policy ");
                out.push_str(&policy.name.name);
                out.push_str(": ");
                out.push_str(&render_dimension_value(&policy.value));
            }
        }
        Decl::Tool(tool) => render_tool(tool, indent, out),
        Decl::Prompt(prompt) => render_prompt(prompt, indent, out),
        Decl::Agent(agent) => render_agent(agent, indent, out),
        Decl::Eval(eval) => {
            push_indent(indent, out);
            out.push_str("eval ");
            out.push_str(&eval.name.name);
            out.push_str(":\n");
            render_block(&eval.body, indent + 1, out);
            if !eval.assertions.is_empty() {
                out.push('\n');
            }
            for (index, assertion) in eval.assertions.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&render_eval_assert(assertion));
            }
        }
        Decl::Test(test) => {
            push_indent(indent, out);
            out.push_str("test ");
            out.push_str(&test.name.name);
            if let Some(path) = &test.trace_fixture {
                out.push_str(" from_trace ");
                out.push_str(&format!("{path:?}"));
            }
            out.push_str(":\n");
            render_block(&test.body, indent + 1, out);
            if !test.assertions.is_empty() {
                out.push('\n');
            }
            for (index, assertion) in test.assertions.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&render_eval_assert(assertion));
            }
        }
        Decl::Fixture(fixture) => render_fixture(fixture, indent, out),
        Decl::Mock(mock) => render_mock(mock, indent, out),
        Decl::Extend(extend) => {
            push_indent(indent, out);
            out.push_str("extend ");
            out.push_str(&extend.type_name.name);
            out.push_str(":\n");
            for (index, method) in extend.methods.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                render_extend_method(method, indent + 1, out);
            }
        }
        Decl::Effect(effect) => {
            push_indent(indent, out);
            out.push_str("effect ");
            out.push_str(&effect.name.name);
            out.push_str(":\n");
            for (index, dimension) in effect.dimensions.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&dimension.name.name);
                out.push_str(": ");
                out.push_str(&render_dimension_value(&dimension.value));
            }
        }
        Decl::Model(model) => {
            push_indent(indent, out);
            out.push_str("model ");
            out.push_str(&model.name.name);
            out.push_str(":\n");
            for (index, field) in model.fields.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&field.name.name);
                out.push_str(": ");
                out.push_str(&render_dimension_value(&field.value));
            }
        }
        Decl::Server(server) => {
            push_indent(indent, out);
            out.push_str("server ");
            out.push_str(&server.name.name);
            out.push_str(":\n");
            for (index, route) in server.routes.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str("route ");
                out.push_str(route.method.as_str());
                out.push(' ');
                out.push_str(&render_string_literal(&route.path));
                if let Some(query_ty) = &route.query_ty {
                    out.push_str(" query ");
                    out.push_str(&render_type_ref(query_ty));
                }
                if let Some(body_ty) = &route.body_ty {
                    out.push_str(" body ");
                    out.push_str(&render_type_ref(body_ty));
                }
                out.push_str(" -> json ");
                out.push_str(&render_type_ref(&route.response.ty));
                if !route.effect_row.effects.is_empty() {
                    out.push_str(" uses ");
                    out.push_str(&render_effect_row_names(&route.effect_row.effects));
                }
                out.push_str(":\n");
                render_block(&route.body, indent + 2, out);
            }
        }
        Decl::Schedule(schedule) => {
            push_indent(indent, out);
            out.push_str("schedule ");
            out.push_str(&render_string_literal(&schedule.cron));
            out.push_str(" zone ");
            out.push_str(&render_string_literal(&schedule.zone));
            out.push_str(" -> ");
            out.push_str(&schedule.target.name);
            out.push('(');
            for (index, arg) in schedule.args.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&render_expr(arg));
            }
            out.push(')');
            if !schedule.effect_row.effects.is_empty() {
                out.push_str(" uses ");
                out.push_str(&render_effect_row_names(&schedule.effect_row.effects));
            }
        }
    }
}

fn render_fixture(fixture: &FixtureDecl, indent: usize, out: &mut String) {
    push_indent(indent, out);
    out.push_str("fixture ");
    out.push_str(&fixture.name.name);
    out.push_str(&render_params(&fixture.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&fixture.return_ty));
    out.push_str(":\n");
    render_block(&fixture.body, indent + 1, out);
}

fn render_mock(mock: &MockDecl, indent: usize, out: &mut String) {
    push_indent(indent, out);
    out.push_str("mock ");
    out.push_str(&mock.target.name);
    out.push_str(&render_params(&mock.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&mock.return_ty));
    if !mock.effect_row.effects.is_empty() {
        out.push_str(" uses ");
        out.push_str(&render_effect_row_names(&mock.effect_row.effects));
    }
    out.push_str(":\n");
    render_block(&mock.body, indent + 1, out);
}

fn render_tool(tool: &ToolDecl, indent: usize, out: &mut String) {
    push_indent(indent, out);
    out.push_str("tool ");
    out.push_str(&tool.name.name);
    out.push_str(&render_params(&tool.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&tool.return_ty));
    if matches!(tool.effect, Effect::Dangerous) {
        out.push_str(" dangerous");
    }
    if !tool.effect_row.effects.is_empty() {
        out.push_str(" uses ");
        out.push_str(&render_effect_row_names(&tool.effect_row.effects));
    }
}

fn render_prompt(prompt: &PromptDecl, indent: usize, out: &mut String) {
    push_indent(indent, out);
    out.push_str("prompt ");
    out.push_str(&prompt.name.name);
    out.push_str(&render_params(&prompt.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&prompt.return_ty));
    if !prompt.effect_row.effects.is_empty() {
        out.push_str(" uses ");
        out.push_str(&render_effect_row_names(&prompt.effect_row.effects));
    }
    out.push_str(":\n");
    if let Some(min_confidence) = prompt.stream.min_confidence {
        push_indent(indent + 1, out);
        out.push_str("with min_confidence ");
        out.push_str(&render_float(min_confidence));
        out.push('\n');
    }
    if let Some(max_tokens) = prompt.stream.max_tokens {
        push_indent(indent + 1, out);
        out.push_str("with max_tokens ");
        out.push_str(&max_tokens.to_string());
        out.push('\n');
    }
    if let Some(policy) = &prompt.stream.backpressure {
        push_indent(indent + 1, out);
        out.push_str("with backpressure ");
        out.push_str(&render_backpressure(policy));
        out.push('\n');
    }
    push_indent(indent + 1, out);
    out.push_str(&render_string_literal(&prompt.template));
}

fn render_agent(agent: &AgentDecl, indent: usize, out: &mut String) {
    for constraint in &agent.constraints {
        push_indent(indent, out);
        out.push_str(&render_constraint(constraint));
        out.push('\n');
    }
    push_indent(indent, out);
    out.push_str("agent ");
    out.push_str(&agent.name.name);
    out.push_str(&render_params(&agent.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&agent.return_ty));
    if !agent.effect_row.effects.is_empty() {
        out.push_str(" uses ");
        out.push_str(&render_effect_row_names(&agent.effect_row.effects));
    }
    out.push_str(":\n");
    render_block(&agent.body, indent + 1, out);
}

fn render_extend_method(method: &ExtendMethod, indent: usize, out: &mut String) {
    push_indent(indent, out);
    match method.visibility {
        Visibility::Private => {}
        Visibility::Public => out.push_str("public "),
        Visibility::PublicPackage => out.push_str("public(package) "),
    }
    match &method.kind {
        ExtendMethodKind::Tool(tool) => render_tool(tool, 0, out),
        ExtendMethodKind::Prompt(prompt) => render_prompt(prompt, 0, out),
        ExtendMethodKind::Agent(agent) => render_agent(agent, 0, out),
    }
}

fn render_block(block: &Block, indent: usize, out: &mut String) {
    for (index, stmt) in block.stmts.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        render_stmt(stmt, indent, out);
    }
}

fn render_stmt(stmt: &Stmt, indent: usize, out: &mut String) {
    match stmt {
        Stmt::Let {
            name, ty, value, ..
        } => {
            push_indent(indent, out);
            out.push_str(&name.name);
            if let Some(ty) = ty {
                out.push_str(": ");
                out.push_str(&render_type_ref(ty));
            }
            out.push_str(" = ");
            out.push_str(&render_expr(value));
        }
        Stmt::Return { value, .. } => {
            push_indent(indent, out);
            out.push_str("return");
            if let Some(value) = value {
                out.push(' ');
                out.push_str(&render_expr(value));
            }
        }
        Stmt::Yield { value, .. } => {
            push_indent(indent, out);
            out.push_str("yield ");
            out.push_str(&render_expr(value));
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            push_indent(indent, out);
            out.push_str("if ");
            out.push_str(&render_expr(cond));
            out.push_str(":\n");
            render_block(then_block, indent + 1, out);
            if let Some(block) = else_block {
                out.push('\n');
                push_indent(indent, out);
                out.push_str("else:\n");
                render_block(block, indent + 1, out);
            }
        }
        Stmt::For {
            var, iter, body, ..
        } => {
            push_indent(indent, out);
            out.push_str("for ");
            out.push_str(&var.name);
            out.push_str(" in ");
            out.push_str(&render_expr(iter));
            out.push_str(":\n");
            render_block(body, indent + 1, out);
        }
        Stmt::Approve { action, .. } => {
            push_indent(indent, out);
            out.push_str("approve ");
            out.push_str(&render_expr(action));
        }
        Stmt::Expr { expr, .. } => {
            push_indent(indent, out);
            out.push_str(&render_expr(expr));
        }
    }
}

fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Literal { value, .. } => render_literal(value),
        Expr::Ident { name, .. } => name.name.clone(),
        Expr::Call { callee, args, .. } => format!(
            "{}({})",
            render_expr(callee),
            args.iter().map(render_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::FieldAccess { target, field, .. } => {
            format!("{}.{}", render_expr(target), field.name)
        }
        Expr::Index { target, index, .. } => {
            format!("{}[{}]", render_expr(target), render_expr(index))
        }
        Expr::BinOp {
            op, left, right, ..
        } => format!(
            "({} {} {})",
            render_expr(left),
            render_binary_op(*op),
            render_expr(right)
        ),
        Expr::UnOp { op, operand, .. } => {
            format!("({}{})", render_unary_op(*op), render_expr(operand))
        }
        Expr::List { items, .. } => format!(
            "[{}]",
            items.iter().map(render_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::TryPropagate { inner, .. } => format!("{}?", render_expr(inner)),
        Expr::TryRetry {
            body,
            attempts,
            backoff,
            ..
        } => format!(
            "try {} on error retry {} times backoff {}",
            render_expr(body),
            attempts,
            render_backoff(*backoff)
        ),
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            // One-line rendering for shrink output. Indented
            // multi-line form can land with a dedicated pretty
            // printer later â€” the shrink harness only needs the
            // shape to be parseable and faithful, not beautiful.
            let mut text = format!("replay {}:", render_expr(trace));
            for arm in arms {
                let capture_tail = match &arm.capture {
                    Some(ident) => format!(" as {}", ident.name),
                    None => String::new(),
                };
                text.push_str(&format!(
                    " when {}{} -> {};",
                    render_replay_pattern(&arm.pattern),
                    capture_tail,
                    render_expr(&arm.body)
                ));
            }
            text.push_str(&format!(" else {}", render_expr(else_body)));
            text
        }
    }
}

fn render_replay_pattern(pattern: &corvid_ast::ReplayPattern) -> String {
    match pattern {
        corvid_ast::ReplayPattern::Llm { prompt, .. } => {
            format!("llm(\"{prompt}\")")
        }
        corvid_ast::ReplayPattern::Tool { tool, arg, .. } => {
            format!("tool(\"{tool}\", {})", render_replay_tool_arg(arg))
        }
        corvid_ast::ReplayPattern::Approve { label, .. } => {
            format!("approve(\"{label}\")")
        }
    }
}

fn render_replay_tool_arg(arg: &corvid_ast::ToolArgPattern) -> String {
    match arg {
        corvid_ast::ToolArgPattern::Wildcard { .. } => "_".into(),
        corvid_ast::ToolArgPattern::StringLit { value, .. } => format!("\"{value}\""),
        corvid_ast::ToolArgPattern::Capture { name, .. } => name.name.clone(),
    }
}

fn render_eval_assert(assertion: &EvalAssert) -> String {
    match assertion {
        EvalAssert::Value {
            expr,
            confidence,
            runs,
            ..
        } => {
            let mut text = format!("assert {}", render_expr(expr));
            if let (Some(confidence), Some(runs)) = (confidence, runs) {
                text.push_str(&format!(
                    " with confidence {} over {} runs",
                    render_float(*confidence),
                    runs
                ));
            }
            text
        }
        EvalAssert::Snapshot { expr, .. } => format!("assert_snapshot {}", render_expr(expr)),
        EvalAssert::Called { tool, .. } => format!("assert called {}", tool.name),
        EvalAssert::Approved { label, .. } => format!("assert approved {}", label.name),
        EvalAssert::Cost { op, bound, .. } => {
            format!(
                "assert cost {} ${}",
                render_binary_op(*op),
                render_float(*bound)
            )
        }
        EvalAssert::Ordering { before, after, .. } => {
            format!("assert called {} before {}", before.name, after.name)
        }
    }
}

fn render_params(params: &[Param]) -> String {
    let rendered: Vec<String> = params
        .iter()
        .map(|param| format!("{}: {}", param.name.name, render_type_ref(&param.ty)))
        .collect();
    format!("({})", rendered.join(", "))
}

fn render_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name, .. } => name.name.clone(),
        TypeRef::Qualified { alias, name, .. } => format!("{}.{}", alias.name, name.name),
        TypeRef::Generic { name, args, .. } => format!(
            "{}<{}>",
            name.name,
            args.iter()
                .map(render_type_ref)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeRef::Weak { inner, effects, .. } => {
            let inner = render_type_ref(inner);
            if let Some(effects) = effects {
                let names: Vec<&str> = effects
                    .effects()
                    .into_iter()
                    .map(|effect| match effect {
                        corvid_ast::WeakEffect::ToolCall => "tool_call",
                        corvid_ast::WeakEffect::Llm => "llm",
                        corvid_ast::WeakEffect::Approve => "approve",
                        corvid_ast::WeakEffect::Human => "human",
                    })
                    .collect();
                format!("Weak<{}, {{{}}}>", inner, names.join(", "))
            } else {
                format!("Weak<{}>", inner)
            }
        }
        TypeRef::Function { params, ret, .. } => format!(
            "({}) -> {}",
            params
                .iter()
                .map(render_type_ref)
                .collect::<Vec<_>>()
                .join(", "),
            render_type_ref(ret)
        ),
    }
}

fn render_literal(literal: &Literal) -> String {
    match literal {
        Literal::Int(value) => value.to_string(),
        Literal::Float(value) => render_float(*value),
        Literal::String(value) => render_string_literal(value),
        Literal::Bool(value) => value.to_string(),
        Literal::Nothing => "nothing".into(),
    }
}

fn render_constraint(constraint: &EffectConstraint) -> String {
    match &constraint.value {
        Some(value) => format!(
            "@{}({})",
            constraint.dimension.name,
            render_dimension_value(value)
        ),
        None => format!("@{}", constraint.dimension.name),
    }
}

fn render_dimension_value(value: &DimensionValue) -> String {
    match value {
        DimensionValue::Bool(value) => value.to_string(),
        DimensionValue::Name(name) => name.clone(),
        DimensionValue::Cost(value) => format!("${}", render_float(*value)),
        DimensionValue::Number(value) => render_float(*value),
        DimensionValue::Streaming { backpressure } => {
            format!("streaming({})", render_backpressure(backpressure))
        }
        DimensionValue::ConfidenceGated {
            threshold, above, ..
        } => {
            format!("{above}_if_confident({})", render_float(*threshold))
        }
    }
}

fn render_effect_row_names(effects: &[corvid_ast::EffectRef]) -> String {
    effects
        .iter()
        .map(|effect| effect.name.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_backpressure(policy: &BackpressurePolicy) -> String {
    match policy {
        policy => policy.label(),
    }
}

fn render_backoff(backoff: Backoff) -> String {
    match backoff {
        Backoff::Linear(value) => format!("linear {value}"),
        Backoff::Exponential(value) => format!("exponential {value}"),
    }
}

fn render_binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::LtEq => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::GtEq => ">=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn render_unary_op(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "not ",
    }
}

fn render_float(value: f64) -> String {
    let text = format!("{value}");
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

fn render_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

fn push_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("    ");
    }
}

pub fn render_corpus_grid(reports: &[DivergenceReport]) -> String {
    let mut lines = vec![
        format!(
            "{:<34} {:<7} {:<7} {:<7} {:<7} {:<9}",
            "program", "check", "interp", "native", "replay", "verdict"
        ),
        format!(
            "{:-<34} {:-<7} {:-<7} {:-<7} {:-<7} {:-<9}",
            "", "", "", "", "", ""
        ),
    ];
    for report in reports {
        let interp = &report.reports[1].profile;
        let cells: Vec<_> = report
            .reports
            .iter()
            .map(|tier| {
                if tier.profile == *interp {
                    "agree"
                } else {
                    "diff"
                }
            })
            .collect();
        let verdict = if report.divergences.is_empty() {
            "ok"
        } else {
            "diverges"
        };
        lines.push(format!(
            "{:<34} {:<7} {:<7} {:<7} {:<7} {:<9}",
            report
                .program
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<unknown>"),
            cells[0],
            cells[1],
            cells[2],
            cells[3],
            verdict
        ));
    }
    lines.join("\n")
}

pub fn render_report(report: &DivergenceReport) -> String {
    let mut lines = vec![format!("{}", report.program.display())];
    for tier in &report.reports {
        lines.push(format!(
            "  {:<11} cost=${:.4} trust={} reversible={} data={} latency={} confidence={:.2}",
            format!("{:?}", tier.tier).to_lowercase(),
            tier.profile.cost,
            render_trust(&tier.profile.trust),
            tier.profile.reversible,
            render_data(&tier.profile.data),
            render_latency(&tier.profile.latency),
            tier.profile.confidence,
        ));
    }
    if report.divergences.is_empty() {
        lines.push("  divergences: none".into());
    } else {
        lines.push(format!("  divergences: {}", report.divergences.len()));
        for divergence in &report.divergences {
            lines.push(format!(
                "    {} [{}]",
                divergence.dimension,
                render_divergence_class(&divergence.classification)
            ));
        }
    }
    lines.join("\n")
}

pub(crate) fn render_trust(level: &TrustLevel) -> String {
    match level {
        TrustLevel::Autonomous => "autonomous".into(),
        TrustLevel::SupervisorRequired => "supervisor_required".into(),
        TrustLevel::HumanRequired => "human_required".into(),
        TrustLevel::Custom(name) => name.clone(),
    }
}

pub(crate) fn render_data(data: &BTreeSet<DataCategory>) -> String {
    if data.is_empty() {
        "none".into()
    } else {
        data.iter()
            .map(|category| category.0.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

pub(crate) fn render_latency(latency: &LatencyLevel) -> String {
    match latency {
        LatencyLevel::Instant => "instant".into(),
        LatencyLevel::Fast => "fast".into(),
        LatencyLevel::Medium => "medium".into(),
        LatencyLevel::Slow => "slow".into(),
        LatencyLevel::Streaming { backpressure } => match backpressure {
            policy => format!("streaming({})", policy.label()),
        },
        LatencyLevel::Custom(name) => name.clone(),
    }
}

fn render_divergence_class(class: &DivergenceClass) -> &'static str {
    match class {
        DivergenceClass::StaticOverapproximated => "static-overapprox",
        DivergenceClass::StaticTooLoose => "static-too-loose",
        DivergenceClass::TierMismatch => "tier-mismatch",
    }
}
