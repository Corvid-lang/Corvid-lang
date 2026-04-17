//! REPL step-through hook: renders VM step events interactively and
//! reads user commands to control execution.

use corvid_vm::{EnvSnapshot, StepAction, StepEvent, StepHook};
use std::io::{self, BufRead, Write};

pub struct ReplStepHook;

#[async_trait::async_trait]
impl StepHook for ReplStepHook {
    async fn on_step(&self, event: &StepEvent) -> StepAction {
        tokio::task::spawn_blocking({
            let event = event.clone();
            move || render_and_prompt(&event)
        })
        .await
        .unwrap_or(StepAction::Resume)
    }
}

fn render_and_prompt(event: &StepEvent) -> StepAction {
    let stderr = io::stderr();
    let mut out = stderr.lock();

    match event {
        StepEvent::BeforeToolCall {
            tool_name,
            args,
            env,
            ..
        } => {
            let _ = writeln!(out, "\n\x1b[36m[step]\x1b[0m tool call: \x1b[1m{tool_name}\x1b[0m");
            render_json_args(&mut out, args);
            render_env(&mut out, env);
            read_step_command(&mut out)
        }

        StepEvent::AfterToolCall {
            tool_name,
            result,
            elapsed_ms,
            ..
        } => {
            let _ = writeln!(
                out,
                "\n\x1b[36m[step]\x1b[0m tool result: \x1b[1m{tool_name}\x1b[0m  \x1b[2m({elapsed_ms}ms)\x1b[0m"
            );
            let _ = writeln!(out, "  result: {}", truncate_json(result, 200));
            read_step_command(&mut out)
        }

        StepEvent::BeforePromptCall {
            prompt_name,
            rendered,
            model,
            env,
            ..
        } => {
            let _ = writeln!(
                out,
                "\n\x1b[35m[step]\x1b[0m prompt call: \x1b[1m{prompt_name}\x1b[0m"
            );
            if let Some(m) = model {
                let _ = writeln!(out, "  model: {m}");
            }
            let _ = writeln!(out, "  rendered: {}", truncate_str(rendered, 200));
            render_env(&mut out, env);
            read_step_command(&mut out)
        }

        StepEvent::AfterPromptCall {
            prompt_name,
            result,
            elapsed_ms,
            ..
        } => {
            let _ = writeln!(
                out,
                "\n\x1b[35m[step]\x1b[0m prompt result: \x1b[1m{prompt_name}\x1b[0m  \x1b[2m({elapsed_ms}ms)\x1b[0m"
            );
            let _ = writeln!(out, "  result: {}", truncate_json(result, 200));
            read_step_command(&mut out)
        }

        StepEvent::BeforeApproval {
            label, args, env, ..
        } => {
            let _ = writeln!(
                out,
                "\n\x1b[33m[step]\x1b[0m \x1b[1mapproval required: {label}\x1b[0m"
            );
            render_json_args(&mut out, args);
            render_env(&mut out, env);
            read_approval_command(&mut out)
        }

        StepEvent::AfterApproval {
            label, approved, ..
        } => {
            let status = if *approved {
                "\x1b[32mapproved\x1b[0m"
            } else {
                "\x1b[31mdenied\x1b[0m"
            };
            let _ = writeln!(
                out,
                "\n\x1b[33m[step]\x1b[0m approval result: \x1b[1m{label}\x1b[0m → {status}"
            );
            read_step_command(&mut out)
        }

        StepEvent::BeforeAgentCall {
            agent_name, args, ..
        } => {
            let _ = writeln!(
                out,
                "\n\x1b[34m[step]\x1b[0m calling agent: \x1b[1m{agent_name}\x1b[0m"
            );
            render_json_args(&mut out, args);
            read_step_command(&mut out)
        }

        StepEvent::AfterAgentCall {
            agent_name, result, ..
        } => {
            let _ = writeln!(
                out,
                "\n\x1b[34m[step]\x1b[0m agent returned: \x1b[1m{agent_name}\x1b[0m"
            );
            let _ = writeln!(out, "  result: {}", truncate_json(result, 200));
            read_step_command(&mut out)
        }

        StepEvent::BeforeStatement { kind, env, .. } => {
            let _ = writeln!(
                out,
                "\n\x1b[2m[step]\x1b[0m {kind}",
                kind = format_stmt_kind(kind)
            );
            render_env(&mut out, env);
            read_step_command(&mut out)
        }

        StepEvent::Completed {
            agent_name,
            ok,
            error,
            ..
        } => {
            if *ok {
                let _ = writeln!(
                    out,
                    "\n\x1b[32m[step]\x1b[0m {agent_name} completed successfully"
                );
            } else {
                let msg = error.as_deref().unwrap_or("unknown error");
                let _ = writeln!(
                    out,
                    "\n\x1b[31m[step]\x1b[0m {agent_name} failed: {msg}"
                );
            }
            StepAction::Resume
        }
    }
}

fn render_json_args(out: &mut impl Write, args: &[serde_json::Value]) {
    if args.is_empty() {
        return;
    }
    let _ = write!(out, "  args: ");
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            let _ = write!(out, ", ");
        }
        let _ = write!(out, "{}", truncate_json(arg, 80));
    }
    let _ = writeln!(out);
}

fn render_env(out: &mut impl Write, env: &EnvSnapshot) {
    if env.locals.is_empty() {
        return;
    }
    let _ = write!(out, "  locals: {{");
    for (i, (name, val)) in env.locals.iter().enumerate() {
        if i > 0 {
            let _ = write!(out, ", ");
        }
        let _ = write!(
            out,
            "{name}: {}",
            truncate_str(&corvid_vm::render_value(val), 60)
        );
    }
    let _ = writeln!(out, "}}");
}

fn format_stmt_kind(kind: &corvid_vm::StmtKind) -> String {
    match kind {
        corvid_vm::StmtKind::Let { name } => format!("let {name} = ..."),
        corvid_vm::StmtKind::Assign { name } => format!("{name} = ..."),
        corvid_vm::StmtKind::Return => "return".into(),
        corvid_vm::StmtKind::If => "if ...".into(),
        corvid_vm::StmtKind::For { var } => format!("for {var} in ..."),
        corvid_vm::StmtKind::Approve { label } => format!("approve {label}"),
        corvid_vm::StmtKind::Expr => "expression".into(),
        corvid_vm::StmtKind::Break => "break".into(),
        corvid_vm::StmtKind::Continue => "continue".into(),
        corvid_vm::StmtKind::Pass => "pass".into(),
    }
}

fn read_step_command(out: &mut impl Write) -> StepAction {
    let _ = write!(
        out,
        "  \x1b[2m[c]ontinue [r]esume [a]bort [o]verride>\x1b[0m "
    );
    let _ = out.flush();
    read_and_parse(parse_step_input)
}

fn read_approval_command(out: &mut impl Write) -> StepAction {
    let _ = write!(
        out,
        "  \x1b[33m[y]es [n]o [c]ontinue(delegate) [a]bort>\x1b[0m "
    );
    let _ = out.flush();
    read_and_parse(parse_approval_input)
}

fn read_and_parse(parser: fn(&str) -> StepAction) -> StepAction {
    let stdin = io::stdin();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        return StepAction::Resume;
    }
    parser(line.trim())
}

fn parse_step_input(input: &str) -> StepAction {
    if input.is_empty() || input == "c" || input == "continue" {
        return StepAction::Continue;
    }
    if input == "r" || input == "resume" {
        return StepAction::Resume;
    }
    if input == "a" || input == "abort" {
        return StepAction::Abort;
    }
    if input == "s" || input == "step" {
        return StepAction::Continue;
    }
    if input == "n" || input == "next" {
        return StepAction::StepOver;
    }
    if let Some(json_str) = input.strip_prefix("o ").or_else(|| input.strip_prefix("override ")) {
        match serde_json::from_str(json_str) {
            Ok(val) => return StepAction::Override(val),
            Err(e) => {
                eprintln!("  \x1b[31minvalid JSON: {e}\x1b[0m");
                return StepAction::Continue;
            }
        }
    }
    StepAction::Continue
}

fn parse_approval_input(input: &str) -> StepAction {
    match input {
        "y" | "yes" | "approve" => StepAction::Approve,
        "n" | "no" | "deny" => StepAction::Deny,
        "c" | "continue" | "" => StepAction::Continue,
        "a" | "abort" => StepAction::Abort,
        _ => {
            eprintln!("  \x1b[2munrecognized; delegating to runtime approver\x1b[0m");
            StepAction::Continue
        }
    }
}

fn truncate_json(val: &serde_json::Value, max_len: usize) -> String {
    let s = val.to_string();
    truncate_str(&s, max_len)
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
