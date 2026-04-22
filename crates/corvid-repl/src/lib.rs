//! Interactive Corvid REPL.

mod file_watch;
mod replay;
mod source_import;
mod step_hook;
mod trace_import;

use std::collections::HashSet;
use std::io::IsTerminal;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use corvid_ast::{Decl, Expr, Stmt};
use corvid_ir::{lower, IrBlock, IrFile, IrStmt};
use corvid_resolve::{ReplResolveSession, ResolvedTurn};
use corvid_runtime::Runtime;
use corvid_syntax::{lex, parse_repl_input, ReplItem};
use corvid_types::{Checked, ReplLocal, ReplSession, Type};
use corvid_vm::{
    render_value, run_agent_stepping, run_agent_with_env, Env, ExecutionTrace, InterpError,
    InterpErrorKind, RecordingHook, ReplayForkHook, StepMode, Value,
};
use replay::ReplaySession;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

const PROMPT: &str = ">>> ";
const CONT_PROMPT: &str = "... ";
const REPL_AGENT_NAME: &str = "__repl_turn__";
const HISTORY_FILE: &str = "history";

#[derive(Clone)]
struct StoredLocal {
    name: String,
    ty: Type,
    value: Value,
}

pub struct Repl {
    resolver: ReplResolveSession,
    typer: ReplSession,
    locals: Vec<StoredLocal>,
    runtime: Runtime,
    tokio: tokio::runtime::Runtime,
    replay: Option<ReplayState>,
    step_mode: StepMode,
    last_trace: Option<ExecutionTrace>,
    last_ir: Option<IrFile>,
    watcher: Option<file_watch::FileWatchManager>,
}

enum ReadTurn {
    Source(String),
    Cancelled,
    Eof,
}

enum TurnEval {
    Completed(Env),
    Cancelled,
    Error(InterpError),
}

struct ReplayState {
    session: ReplaySession,
    current: Option<usize>,
    next: usize,
}

impl Repl {
    pub fn new() -> io::Result<Self> {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(io::Error::other)?;
        Ok(Self {
            resolver: ReplResolveSession::new(),
            typer: ReplSession::new(),
            locals: Vec::new(),
            runtime: Runtime::builder().build(),
            tokio,
            replay: None,
            step_mode: StepMode::Run,
            last_trace: None,
            last_ir: None,
            watcher: None,
        })
    }

    /// Read REPL turns until EOF. Multi-line mode begins when a turn's
    /// first line ends in `:` and finishes on a blank line.
    pub fn run<R: BufRead, W: Write>(input: R, output: &mut W) -> io::Result<()> {
        let mut repl = Self::new()?;
        repl.run_loop(input, output)
    }

    pub fn run_stdio() -> io::Result<()> {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            let mut repl = Self::new()?;
            repl.run_interactive()
        } else {
            let stdin = io::stdin();
            let stdout = io::stdout();
            let reader = io::BufReader::new(stdin.lock());
            let mut writer = io::BufWriter::new(stdout.lock());
            Self::run(reader, &mut writer)
        }
    }

    fn run_loop<R: BufRead, W: Write>(&mut self, mut input: R, output: &mut W) -> io::Result<()> {
        while let Some(source) = self.read_turn(&mut input, output)? {
            self.process_source(&source, output, false)?;
        }
        Ok(())
    }

    fn run_interactive(&mut self) -> io::Result<()> {
        let mut editor = DefaultEditor::new().map_err(io::Error::other)?;
        let history_path = history_path()?;
        if history_path.exists() {
            let _ = editor.load_history(&history_path);
        }

        loop {
            self.check_hot_reload(&mut io::stderr().lock())?;
            match self.read_turn_interactive(&mut editor)? {
                ReadTurn::Source(source) => {
                    self.process_source(&source, &mut io::stdout().lock(), true)?;
                    let entry = source.trim_end();
                    if !entry.trim().is_empty() {
                        let _ = editor.add_history_entry(entry);
                    }
                }
                ReadTurn::Cancelled => {
                    println!("^C");
                }
                ReadTurn::Eof => break,
            }
        }

        ensure_parent_dir(&history_path)?;
        let _ = editor.save_history(&history_path);
        Ok(())
    }

    fn read_turn<R: BufRead, W: Write>(
        &self,
        input: &mut R,
        output: &mut W,
    ) -> io::Result<Option<String>> {
        let mut source = String::new();
        let mut line = String::new();

        loop {
            output.write_all(if source.is_empty() { PROMPT.as_bytes() } else { CONT_PROMPT.as_bytes() })?;
            output.flush()?;

            line.clear();
            let read = input.read_line(&mut line)?;
            if read == 0 {
                if source.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(source));
            }

            if source.is_empty() && line.trim().is_empty() && self.replay.is_none() {
                continue;
            }

            let is_blank = line.trim().is_empty();
            source.push_str(&line);

            if source.lines().next().is_some_and(|first| !first.trim_end().ends_with(':')) {
                return Ok(Some(source));
            }

            if is_blank {
                return Ok(Some(source));
            }
        }
    }

    fn read_turn_interactive(&self, editor: &mut DefaultEditor) -> io::Result<ReadTurn> {
        let mut source = String::new();

        loop {
            let prompt = if source.is_empty() { PROMPT } else { CONT_PROMPT };
            match editor.readline(prompt) {
                Ok(line) => {
                    let is_blank = line.trim().is_empty();
                    if source.is_empty() && is_blank && self.replay.is_none() {
                        continue;
                    }

                    source.push_str(&line);
                    source.push('\n');

                    if source
                        .lines()
                        .next()
                        .is_some_and(|first| !first.trim_end().ends_with(':'))
                    {
                        return Ok(ReadTurn::Source(source));
                    }

                    if is_blank {
                        return Ok(ReadTurn::Source(source));
                    }
                }
                Err(ReadlineError::Interrupted) => return Ok(ReadTurn::Cancelled),
                Err(ReadlineError::Eof) => {
                    if source.is_empty() {
                        return Ok(ReadTurn::Eof);
                    }
                    return Ok(ReadTurn::Source(source));
                }
                Err(err) => return Err(io::Error::other(err)),
            }
        }
    }

    fn process_source<W: Write>(
        &mut self,
        source: &str,
        output: &mut W,
        allow_interrupt: bool,
    ) -> io::Result<()> {
        if self.try_handle_command(source, output)? {
            return Ok(());
        }
        if self.replay.is_some() {
            writeln!(output, "replay mode accepts only :step, :run, :show, :where, and :quit")?;
            output.flush()?;
            return Ok(());
        }
        if source.trim().is_empty() {
            return Ok(());
        }
        match lex(source) {
            Ok(tokens) => match parse_repl_input(&tokens) {
                Ok(item) => self.handle_item(item, output, allow_interrupt)?,
                Err(errors) => self.write_errors(errors, output)?,
            },
            Err(errors) => self.write_errors(errors, output)?,
        }
        Ok(())
    }

    fn try_handle_command<W: Write>(&mut self, source: &str, output: &mut W) -> io::Result<bool> {
        let trimmed = source.trim();

        if self.replay.is_some() && trimmed.is_empty() {
            self.step_replay(1, output)?;
            return Ok(true);
        }

        if !trimmed.starts_with(':') {
            return Ok(false);
        }

        let mut parts = trimmed.split_whitespace();
        let command = parts.next().unwrap_or_default();
        match command {
            ":replay" => {
                let path = trimmed[command.len()..].trim();
                if path.is_empty() {
                    writeln!(output, "usage: :replay <trace-path>")?;
                    output.flush()?;
                    return Ok(true);
                }
                self.enter_replay(path, output)?;
                Ok(true)
            }
            ":step" | ":s" => {
                let rest = trimmed[command.len()..].trim();
                let count = if rest.is_empty() {
                    1
                } else {
                    rest.parse::<usize>().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "invalid step count")
                    })?
                };
                self.step_replay(count, output)?;
                Ok(true)
            }
            ":run" => {
                self.run_replay(output)?;
                Ok(true)
            }
            ":show" => {
                self.show_replay(output)?;
                Ok(true)
            }
            ":where" => {
                self.where_replay(output)?;
                Ok(true)
            }
            ":quit" | ":q" => {
                self.quit_replay(output)?;
                Ok(true)
            }
            ":stepon" => {
                self.step_mode = StepMode::Boundary;
                writeln!(output, "step-through enabled (pauses at tool/prompt/approval boundaries)")?;
                output.flush()?;
                Ok(true)
            }
            ":stepinto" => {
                self.step_mode = StepMode::Statement;
                writeln!(output, "step-through enabled (pauses at every statement)")?;
                output.flush()?;
                Ok(true)
            }
            ":stepoff" => {
                self.step_mode = StepMode::Run;
                writeln!(output, "step-through disabled")?;
                output.flush()?;
                Ok(true)
            }
            ":whatif" => {
                let rest = trimmed[command.len()..].trim();
                let (target, json_str) = match rest.split_once(" returns ") {
                    Some((t, j)) => (t.trim(), j.trim()),
                    None => {
                        writeln!(output, "usage: :whatif <tool_or_prompt> returns <json>")?;
                        output.flush()?;
                        return Ok(true);
                    }
                };
                match serde_json::from_str::<serde_json::Value>(json_str) {
                    Ok(val) => self.eval_whatif(target, val, output)?,
                    Err(e) => {
                        writeln!(output, "invalid JSON: {e}")?;
                        output.flush()?;
                    }
                }
                Ok(true)
            }
            ":trace" => {
                match &self.last_trace {
                    Some(trace) => {
                        let boundaries = trace.boundaries();
                        if boundaries.is_empty() {
                            writeln!(output, "last trace: {} checkpoint(s), no boundary events", trace.len())?;
                        } else {
                            writeln!(output, "last trace: {} checkpoint(s), {} boundary event(s):", trace.len(), boundaries.len())?;
                            for cp in &boundaries {
                                let label = match &cp.event {
                                    corvid_vm::StepEvent::BeforeToolCall { tool_name, .. } => format!("  [{:>3}] tool call: {tool_name}", cp.index),
                                    corvid_vm::StepEvent::AfterToolCall { tool_name, elapsed_ms, .. } => format!("  [{:>3}] tool result: {tool_name} ({elapsed_ms}ms)", cp.index),
                                    corvid_vm::StepEvent::BeforePromptCall { prompt_name, .. } => format!("  [{:>3}] prompt call: {prompt_name}", cp.index),
                                    corvid_vm::StepEvent::AfterPromptCall { prompt_name, elapsed_ms, .. } => format!("  [{:>3}] prompt result: {prompt_name} ({elapsed_ms}ms)", cp.index),
                                    corvid_vm::StepEvent::BeforeApproval { label, .. } => format!("  [{:>3}] approval: {label}", cp.index),
                                    corvid_vm::StepEvent::AfterApproval { label, approved, .. } => format!("  [{:>3}] approval result: {label} -> {}", cp.index, if *approved { "yes" } else { "no" }),
                                    corvid_vm::StepEvent::BeforeAgentCall { agent_name, .. } => format!("  [{:>3}] agent call: {agent_name}", cp.index),
                                    corvid_vm::StepEvent::AfterAgentCall { agent_name, .. } => format!("  [{:>3}] agent result: {agent_name}", cp.index),
                                    _ => continue,
                                };
                                writeln!(output, "{label}")?;
                            }
                        }
                    }
                    None => {
                        writeln!(output, "no execution trace available (run with :stepon first)")?;
                    }
                }
                output.flush()?;
                Ok(true)
            }
            ":help" | ":h" => {
                self.cmd_help(output)?;
                Ok(true)
            }
            ":type" | ":t" => {
                let rest = trimmed[command.len()..].trim();
                if rest.is_empty() {
                    writeln!(output, "usage: :type <expression>")?;
                    output.flush()?;
                } else {
                    self.cmd_type(rest, output)?;
                }
                Ok(true)
            }
            ":reset" => {
                let rest = trimmed[command.len()..].trim();
                self.cmd_reset(rest, output)?;
                Ok(true)
            }
            ":inspect" | ":i" => {
                let rest = trimmed[command.len()..].trim();
                if rest.is_empty() {
                    writeln!(output, "usage: :inspect <name>")?;
                    output.flush()?;
                } else {
                    self.cmd_inspect(rest, output)?;
                }
                Ok(true)
            }
            ":locals" | ":env" => {
                self.cmd_locals(output)?;
                Ok(true)
            }
            ":cost" => {
                let rest = trimmed[command.len()..].trim();
                if rest.is_empty() {
                    writeln!(output, "usage: :cost <agent_name>")?;
                    output.flush()?;
                } else {
                    self.cmd_cost(rest, output)?;
                }
                Ok(true)
            }
            ":import-trace" => {
                let path = trimmed[command.len()..].trim().trim_matches('"');
                if path.is_empty() {
                    writeln!(output, "usage: :import-trace <trace.jsonl>")?;
                    output.flush()?;
                } else {
                    self.cmd_import_trace(path, output)?;
                }
                Ok(true)
            }
            ":import" => {
                let rest = trimmed[command.len()..].trim();
                if rest.is_empty() {
                    writeln!(output, "usage: :import \"<path.cor>\" or :import <name> from \"<path.cor>\"")?;
                    output.flush()?;
                } else {
                    self.cmd_import_source(rest, output)?;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_item<W: Write>(
        &mut self,
        item: ReplItem,
        output: &mut W,
        allow_interrupt: bool,
    ) -> io::Result<()> {
        match item {
            ReplItem::Decl(decl) => self.handle_decl_turn(decl, output),
            ReplItem::Stmt(stmt) => self.handle_stmt_turn(stmt, output, allow_interrupt),
            ReplItem::Expr(expr) => self.handle_expr_turn(expr, output, allow_interrupt),
        }
    }

    fn handle_decl_turn<W: Write>(&mut self, decl: Decl, output: &mut W) -> io::Result<()> {
        if matches!(decl, Decl::Import(_)) {
            writeln!(output, "imports are not supported in `corvid repl` yet")?;
            output.flush()?;
            return Ok(());
        }

        let name = corvid_resolve::decl_name(&decl).map(|s| s.to_string());
        let is_redefine = name
            .as_deref()
            .is_some_and(|n| self.resolver.has_decl(n));

        if is_redefine {
            match self.resolver.redefine_decl(decl) {
                Some(result) => {
                    if !result.resolved.resolved.errors.is_empty() {
                        self.write_resolve_errors(&result.resolved, output)?;
                        return Ok(());
                    }

                    let checked = self.typer.typecheck_decl_turn(
                        &result.resolved.file,
                        &result.resolved.resolved,
                    );
                    if self.write_type_errors(&checked, output)? {
                        return Ok(());
                    }

                    writeln!(
                        output,
                        "\x1b[33m(redefined {kind} `{name}`)\x1b[0m",
                        kind = result.new_kind,
                        name = result.name,
                    )?;

                    if !result.affected_names.is_empty() {
                        writeln!(
                            output,
                            "  \x1b[2maffected: {}\x1b[0m",
                            result.affected_names.join(", ")
                        )?;
                    }
                    output.flush()?;
                }
                None => {
                    writeln!(output, "failed to redefine declaration")?;
                    output.flush()?;
                }
            }
        } else {
            let turn = self.resolver.resolve_decl_turn(decl.clone());
            if self.write_resolve_errors(&turn, output)? {
                return Ok(());
            }

            let checked = self.typer.typecheck_decl_turn(&turn.file, &turn.resolved);
            if self.write_type_errors(&checked, output)? {
                return Ok(());
            }

            self.resolver.commit_decl(decl);
        }
        Ok(())
    }

    fn handle_stmt_turn<W: Write>(
        &mut self,
        stmt: Stmt,
        output: &mut W,
        allow_interrupt: bool,
    ) -> io::Result<()> {
        let base = self.resolver.resolve_current();
        if self.write_resolve_errors(&base, output)? {
            return Ok(());
        }

        let build = self.typer.build_stmt_turn(stmt, &base.resolved.symbols);
        let turn = self.resolver.resolve_agent_turn(build.agent.clone());
        if self.write_resolve_errors(&turn, output)? {
            return Ok(());
        }

        let checked_turn = self.typer.typecheck_turn(&turn.file, &turn.resolved, build);
        if self.write_type_errors(&checked_turn.checked, output)? {
            return Ok(());
        }

        let ir = lower(&turn.file, &turn.resolved, &checked_turn.checked);
        let env = match self.eval_turn(&ir, output, allow_interrupt)? {
            Some(env) => env,
            None => return Ok(()),
        };

        self.commit_turn_locals(
            &ir,
            &checked_turn.checked,
            &env,
            checked_turn.build.result_name.as_deref(),
        );
        Ok(())
    }

    fn handle_expr_turn<W: Write>(
        &mut self,
        expr: Expr,
        output: &mut W,
        allow_interrupt: bool,
    ) -> io::Result<()> {
        let base = self.resolver.resolve_current();
        if self.write_resolve_errors(&base, output)? {
            return Ok(());
        }

        let build = self.typer.build_expr_turn(expr, &base.resolved.symbols);
        let turn = self.resolver.resolve_agent_turn(build.agent.clone());
        if self.write_resolve_errors(&turn, output)? {
            return Ok(());
        }

        let checked_turn = self.typer.typecheck_turn(&turn.file, &turn.resolved, build);
        if self.write_type_errors(&checked_turn.checked, output)? {
            return Ok(());
        }

        let ir = lower(&turn.file, &turn.resolved, &checked_turn.checked);
        let env = match self.eval_turn(&ir, output, allow_interrupt)? {
            Some(env) => env,
            None => return Ok(()),
        };

        let result = self.commit_turn_locals(
            &ir,
            &checked_turn.checked,
            &env,
            checked_turn.build.result_name.as_deref(),
        );
        if let Some(value) = result {
            writeln!(output, "{}", render_value(&value))?;
            output.flush()?;
        }
        Ok(())
    }

    fn eval_turn<W: Write>(
        &mut self,
        ir: &IrFile,
        output: &mut W,
        allow_interrupt: bool,
    ) -> io::Result<Option<Env>> {
        self.last_ir = Some(ir.clone());
        let args: Vec<Value> = self.locals.iter().map(|local| local.value.clone()).collect();
        let step_mode = self.step_mode;

        let outcome = self.tokio.block_on(async {
            if step_mode != StepMode::Run {
                let hook: std::sync::Arc<dyn corvid_vm::StepHook> =
                    std::sync::Arc::new(step_hook::ReplStepHook);
                match run_agent_stepping(
                    ir, REPL_AGENT_NAME, args, &self.runtime,
                    hook, step_mode,
                ).await {
                    Ok((_, env)) => TurnEval::Completed(env),
                    Err(error) => TurnEval::Error(error),
                }
            } else if allow_interrupt {
                tokio::select! {
                    result = run_agent_with_env(ir, REPL_AGENT_NAME, args, &self.runtime) => {
                        match result {
                            Ok((_value, env)) => TurnEval::Completed(env),
                            Err(error) => TurnEval::Error(error),
                        }
                    }
                    signal = tokio::signal::ctrl_c() => {
                        match signal {
                            Ok(()) => TurnEval::Cancelled,
                            Err(error) => TurnEval::Error(InterpError::new(
                                InterpErrorKind::Other(format!("failed to listen for ctrl-c: {error}")),
                                corvid_ast::Span::new(0, 0),
                            )),
                        }
                    }
                }
            } else {
                match run_agent_with_env(ir, REPL_AGENT_NAME, args, &self.runtime).await {
                    Ok((_value, env)) => TurnEval::Completed(env),
                    Err(error) => TurnEval::Error(error),
                }
            }
        });

        match outcome {
            TurnEval::Completed(env) => Ok(Some(env)),
            TurnEval::Cancelled => {
                writeln!(output, "^C")?;
                output.flush()?;
                Ok(None)
            }
            TurnEval::Error(error) => {
                writeln!(output, "{error}")?;
                output.flush()?;
                Ok(None)
            }
        }
    }

    fn eval_turn_recording<W: Write>(
        &mut self,
        ir: &IrFile,
        output: &mut W,
        step_mode: StepMode,
    ) -> io::Result<(Option<Env>, Option<ExecutionTrace>)> {
        self.last_ir = Some(ir.clone());
        let args: Vec<Value> = self.locals.iter().map(|local| local.value.clone()).collect();

        let trace_store = std::sync::Arc::new(std::sync::Mutex::new(ExecutionTrace::default()));
        let trace_clone = std::sync::Arc::clone(&trace_store);

        let outcome = self.tokio.block_on(async {
            let inner: std::sync::Arc<dyn corvid_vm::StepHook> =
                std::sync::Arc::new(step_hook::ReplStepHook);
            let recording = std::sync::Arc::new(RecordingHook::new(inner));
            let trace_ref = recording.trace_ref();
            let result = run_agent_stepping(
                ir, REPL_AGENT_NAME, args, &self.runtime,
                recording, step_mode,
            ).await;
            let trace = trace_ref.lock().unwrap().clone();
            *trace_clone.lock().unwrap() = trace;
            match result {
                Ok((_, env)) => TurnEval::Completed(env),
                Err(error) => TurnEval::Error(error),
            }
        });

        let trace = trace_store.lock().unwrap().clone();
        self.last_trace = Some(trace.clone());

        match outcome {
            TurnEval::Completed(env) => Ok((Some(env), Some(trace))),
            TurnEval::Cancelled => {
                writeln!(output, "^C")?;
                output.flush()?;
                Ok((None, Some(trace)))
            }
            TurnEval::Error(error) => {
                writeln!(output, "{error}")?;
                output.flush()?;
                Ok((None, Some(trace)))
            }
        }
    }

    fn eval_whatif<W: Write>(
        &mut self,
        target: &str,
        override_json: serde_json::Value,
        output: &mut W,
    ) -> io::Result<()> {
        let trace = match self.last_trace.as_ref() {
            Some(t) => t.clone(),
            None => {
                writeln!(output, "no execution trace available (run with :stepon first)")?;
                output.flush()?;
                return Ok(());
            }
        };
        let ir = match self.last_ir.as_ref() {
            Some(ir) => ir.clone(),
            None => {
                writeln!(output, "no IR available from the last execution")?;
                output.flush()?;
                return Ok(());
            }
        };

        let fork_at = trace
            .find_tool_call(target)
            .or_else(|| trace.find_prompt_call(target))
            .or_else(|| trace.find_approval(target));

        let fork_at = match fork_at {
            Some(idx) => idx,
            None => {
                writeln!(output, "no tool, prompt, or approval named `{target}` in the last trace")?;
                output.flush()?;
                return Ok(());
            }
        };

        writeln!(
            output,
            "forking at checkpoint {fork_at}, overriding `{target}` with {}",
            serde_json::to_string(&override_json).unwrap_or_default()
        )?;
        output.flush()?;

        let args: Vec<Value> = self.locals.iter().map(|local| local.value.clone()).collect();
        let outcome = self.tokio.block_on(async {
            let live: std::sync::Arc<dyn corvid_vm::StepHook> =
                std::sync::Arc::new(step_hook::ReplStepHook);
            let fork_hook = std::sync::Arc::new(ReplayForkHook::new(
                trace, fork_at, Some(override_json), live,
            ));
            match run_agent_stepping(
                &ir, REPL_AGENT_NAME, args, &self.runtime,
                fork_hook, StepMode::Boundary,
            ).await {
                Ok((_, env)) => TurnEval::Completed(env),
                Err(error) => TurnEval::Error(error),
            }
        });

        match outcome {
            TurnEval::Completed(env) => {
                // For whatif runs we show the return value directly without
                // committing locals — the session state stays as it was
                // before the counterfactual.
                let agent = ir.agents.iter().find(|a| a.name == REPL_AGENT_NAME);
                if let Some(agent) = agent {
                    if let Some(last_param) = agent.params.last() {
                        if let Some(val) = env.lookup(last_param.local_id) {
                            writeln!(output, "= {}", render_value(&val))?;
                            output.flush()?;
                        }
                    }
                }
            }
            TurnEval::Cancelled => {
                writeln!(output, "^C")?;
                output.flush()?;
            }
            TurnEval::Error(error) => {
                writeln!(output, "{error}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    fn cmd_help<W: Write>(&self, output: &mut W) -> io::Result<()> {
        let step_status = match self.step_mode {
            StepMode::Run => "off",
            StepMode::Boundary => "on (boundary)",
            StepMode::Statement => "on (statement)",
        };

        writeln!(output, "\x1b[1mCorvid REPL\x1b[0m")?;
        writeln!(output)?;
        writeln!(output, "\x1b[1mExecution:\x1b[0m")?;
        writeln!(output, "  <expression>     evaluate and print result")?;
        writeln!(output, "  <statement>      execute (let, if, for, return)")?;
        writeln!(output, "  <declaration>    define agent/type/tool/prompt")?;
        writeln!(output)?;
        writeln!(output, "\x1b[1mStep-through:\x1b[0m  (current: {step_status})")?;
        writeln!(output, "  :stepon          pause at tool/prompt/approval boundaries")?;
        writeln!(output, "  :stepinto        pause at every statement")?;
        writeln!(output, "  :stepoff         normal execution")?;
        writeln!(output, "  :trace           show last execution trace")?;
        writeln!(output, "  :whatif <name> returns <json>")?;
        writeln!(output, "                   re-run with a counterfactual result")?;
        writeln!(output)?;
        writeln!(output, "\x1b[1mInspection:\x1b[0m")?;
        writeln!(output, "  :type <expr>     show inferred type without executing")?;
        writeln!(output, "  :cost <agent>    show worst-case multi-dimensional cost")?;
        writeln!(output, "  :inspect <name>  show declaration details + dependencies")?;
        writeln!(output, "  :locals          show current local bindings")?;
        writeln!(output)?;
        writeln!(output, "\x1b[1mImport:\x1b[0m")?;
        writeln!(output, "  :import \"path.cor\"          import all declarations from file")?;
        writeln!(output, "  :import name from \"path.cor\" import one decl + dependencies")?;
        writeln!(output, "  :import-trace \"trace.jsonl\"  mock tools/prompts from trace")?;
        writeln!(output)?;
        writeln!(output, "\x1b[1mSession:\x1b[0m")?;
        writeln!(output, "  :reset           clear everything (locals + declarations)")?;
        writeln!(output, "  :reset locals    clear local values, keep declarations")?;
        writeln!(output, "  :help            this message")?;
        writeln!(output)?;
        writeln!(output, "\x1b[1mReplay:\x1b[0m")?;
        writeln!(output, "  :replay <path>   load a JSONL trace file")?;
        writeln!(output, "  :step [n]        advance n replay steps")?;
        writeln!(output, "  :run             run remaining replay steps")?;
        writeln!(output, "  :show            re-display current replay step")?;
        writeln!(output, "  :where           show replay position")?;
        writeln!(output, "  :quit            exit replay mode")?;
        writeln!(output)?;

        // Session state summary
        let decls = self.resolver.decls();
        let agents: Vec<&str> = decls.iter().filter_map(|d| match d {
            Decl::Agent(a) => Some(a.name.name.as_str()),
            _ => None,
        }).collect();
        let types: Vec<&str> = decls.iter().filter_map(|d| match d {
            Decl::Type(t) => Some(t.name.name.as_str()),
            _ => None,
        }).collect();
        let tools: Vec<&str> = decls.iter().filter_map(|d| match d {
            Decl::Tool(t) => Some(t.name.name.as_str()),
            _ => None,
        }).collect();
        let prompts: Vec<&str> = decls.iter().filter_map(|d| match d {
            Decl::Prompt(p) => Some(p.name.name.as_str()),
            _ => None,
        }).collect();

        if !agents.is_empty() || !types.is_empty() || !tools.is_empty() || !prompts.is_empty() {
            writeln!(output, "\x1b[1mSession:\x1b[0m")?;
            if !agents.is_empty() {
                writeln!(output, "  agents:  {}", agents.join(", "))?;
            }
            if !types.is_empty() {
                writeln!(output, "  types:   {}", types.join(", "))?;
            }
            if !tools.is_empty() {
                writeln!(output, "  tools:   {}", tools.join(", "))?;
            }
            if !prompts.is_empty() {
                writeln!(output, "  prompts: {}", prompts.join(", "))?;
            }
        }
        if !self.locals.is_empty() {
            writeln!(output, "  locals:  {}", self.locals.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", "))?;
        }

        output.flush()
    }

    fn cmd_type<W: Write>(&mut self, expr_src: &str, output: &mut W) -> io::Result<()> {
        let tokens = match lex(expr_src) {
            Ok(t) => t,
            Err(errors) => {
                self.write_errors(errors, output)?;
                return Ok(());
            }
        };
        let item = match parse_repl_input(&tokens) {
            Ok(ReplItem::Expr(expr)) => expr,
            Ok(_) => {
                writeln!(output, ":type expects an expression, not a statement or declaration")?;
                output.flush()?;
                return Ok(());
            }
            Err(errors) => {
                self.write_errors(errors, output)?;
                return Ok(());
            }
        };

        let base = self.resolver.resolve_current();
        if !base.resolved.errors.is_empty() {
            self.write_resolve_errors(&base, output)?;
            return Ok(());
        }

        let build = self.typer.build_expr_turn(item, &base.resolved.symbols);
        let turn = self.resolver.resolve_agent_turn(build.agent.clone());
        if !turn.resolved.errors.is_empty() {
            self.write_resolve_errors(&turn, output)?;
            return Ok(());
        }

        let checked_turn = self.typer.typecheck_turn(&turn.file, &turn.resolved, build);
        if !checked_turn.checked.errors.is_empty() {
            self.write_type_errors(&checked_turn.checked, output)?;
            return Ok(());
        }

        // Find the result type. The synthetic agent wraps the expression
        // in a `let __repl_result__ = <expr>`, so the result type is the
        // type assigned to that local.
        let result_name = checked_turn.build.result_name.as_deref();
        let result_ty = if let Some(_name) = result_name {
            checked_turn
                .checked
                .local_types
                .values()
                .next()
                .cloned()
        } else {
            None
        };

        match result_ty {
            Some(ty) => {
                let display = display_type_rich(&ty, &turn.resolved.symbols);
                writeln!(output, "{display}")?;
            }
            None => {
                writeln!(output, "<could not infer type>")?;
            }
        }
        output.flush()
    }

    fn cmd_reset<W: Write>(&mut self, scope: &str, output: &mut W) -> io::Result<()> {
        match scope {
            "" | "all" => {
                self.resolver = corvid_resolve::ReplResolveSession::new();
                self.typer = ReplSession::new();
                self.locals.clear();
                self.last_trace = None;
                self.last_ir = None;
                writeln!(output, "session reset (all declarations and locals cleared)")?;
            }
            "locals" => {
                self.locals.clear();
                self.typer.commit_locals(Vec::new());
                writeln!(output, "locals cleared (declarations preserved)")?;
            }
            other => {
                writeln!(output, "unknown reset scope `{other}` (use :reset, :reset all, or :reset locals)")?;
            }
        }
        output.flush()
    }

    fn cmd_inspect<W: Write>(&self, name: &str, output: &mut W) -> io::Result<()> {
        // Check locals first
        if let Some(local) = self.locals.iter().find(|l| l.name == name) {
            writeln!(output, "\x1b[1m{name}\x1b[0m: {} = {}",
                local.ty.display_name(),
                render_value(&local.value)
            )?;
            output.flush()?;
            return Ok(());
        }

        // Check declarations
        let decls = self.resolver.decls();
        let decl = decls.iter().find(|d| corvid_resolve::decl_name(d) == Some(name));
        let decl = match decl {
            Some(d) => d,
            None => {
                writeln!(output, "`{name}` is not defined in this session")?;
                output.flush()?;
                return Ok(());
            }
        };

        match decl {
            Decl::Agent(a) => {
                let params: Vec<String> = a.params.iter().map(|p| {
                    format!("{}: {}", p.name.name, format_typeref(&p.ty))
                }).collect();
                writeln!(output, "\x1b[1magent {name}\x1b[0m({}) -> {}",
                    params.join(", "),
                    format_typeref(&a.return_ty),
                )?;
                writeln!(output, "  body: {} statement(s)", a.body.stmts.len())?;
            }
            Decl::Type(t) => {
                writeln!(output, "\x1b[1mtype {name}\x1b[0m:")?;
                for field in &t.fields {
                    writeln!(output, "    {}: {}", field.name.name, format_typeref(&field.ty))?;
                }
            }
            Decl::Tool(t) => {
                let params: Vec<String> = t.params.iter().map(|p| {
                    format!("{}: {}", p.name.name, format_typeref(&p.ty))
                }).collect();
                let effect = match t.effect {
                    corvid_ast::Effect::Safe => "",
                    corvid_ast::Effect::Dangerous => " dangerous",
                };
                writeln!(output, "\x1b[1mtool {name}\x1b[0m({}) -> {}{effect}",
                    params.join(", "),
                    format_typeref(&t.return_ty),
                )?;
            }
            Decl::Prompt(p) => {
                let params: Vec<String> = p.params.iter().map(|pp| {
                    format!("{}: {}", pp.name.name, format_typeref(&pp.ty))
                }).collect();
                writeln!(output, "\x1b[1mprompt {name}\x1b[0m({}) -> {}",
                    params.join(", "),
                    format_typeref(&p.return_ty),
                )?;
                writeln!(output, "  template: \"{}\"",
                    if p.template.len() > 80 {
                        format!("{}...", &p.template[..80])
                    } else {
                        p.template.clone()
                    }
                )?;
            }
            _ => {
                writeln!(output, "{name}: (no details available)")?;
            }
        }

        // Show dependency info
        let turn = self.resolver.resolve_current();
        let dep_graph = corvid_resolve::build_dep_graph(&turn.file, &turn.resolved);
        if let Some(def_id) = turn.resolved.symbols.lookup_def(name) {
            let uses = dep_graph.dependencies_of(def_id);
            let used_by = dep_graph.dependents_of(def_id);

            if !uses.is_empty() {
                let names: Vec<String> = uses.iter().filter_map(|&id| {
                    Some(turn.resolved.symbols.get(id).name.clone())
                }).collect();
                writeln!(output, "  \x1b[2muses: {}\x1b[0m", names.join(", "))?;
            }
            if !used_by.is_empty() {
                let names: Vec<String> = used_by.iter().filter_map(|&id| {
                    Some(turn.resolved.symbols.get(id).name.clone())
                }).collect();
                writeln!(output, "  \x1b[2mused by: {}\x1b[0m", names.join(", "))?;
            }
        }

        output.flush()
    }

    fn cmd_cost<W: Write>(&self, name: &str, output: &mut W) -> io::Result<()> {
        let turn = self.resolver.resolve_current();
        if !turn.resolved.errors.is_empty() {
            self.write_resolve_errors(&turn, output)?;
            return Ok(());
        }

        let decl = turn
            .file
            .decls
            .iter()
            .find(|decl| corvid_resolve::decl_name(decl) == Some(name));
        let Some(Decl::Agent(agent)) = decl else {
            writeln!(output, "`{name}` is not a known agent in this session")?;
            output.flush()?;
            return Ok(());
        };

        let effect_decls: Vec<_> = turn
            .file
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Effect(effect) => Some(effect.clone()),
                _ => None,
            })
            .collect();
        let registry = corvid_types::EffectRegistry::from_decls(&effect_decls);
        let Some(estimate) =
            corvid_types::compute_worst_case_cost(&turn.file, &turn.resolved, &registry, name)
        else {
            writeln!(output, "no cost estimate available for `{name}`")?;
            output.flush()?;
            return Ok(());
        };

        writeln!(
            output,
            "{}",
            corvid_types::render_cost_tree(&estimate.tree, Some(&agent.constraints))
        )?;
        if !estimate.warnings.is_empty() {
            writeln!(output)?;
            for warning in estimate.warnings {
                match warning.kind {
                    corvid_types::CostWarningKind::UnboundedLoop { message, .. } => {
                        writeln!(output, "warning: {message}")?;
                    }
                }
            }
        }
        output.flush()
    }

    fn cmd_import_trace<W: Write>(&mut self, path: &str, output: &mut W) -> io::Result<()> {
        let path = std::path::Path::new(path);
        let mocks = match trace_import::load_trace(path) {
            Ok(m) => m,
            Err(e) => {
                writeln!(output, "{e}")?;
                output.flush()?;
                return Ok(());
            }
        };

        if mocks.tools.is_empty() && mocks.prompts.is_empty() {
            writeln!(output, "trace contains no tool or prompt calls to import")?;
            output.flush()?;
            return Ok(());
        }

        let result = trace_import::build_import(&mocks);

        // Add generated declarations to the session.
        for decl in &result.decls {
            let name = corvid_resolve::decl_name(decl).map(|s| s.to_string());
            if let Some(ref n) = name {
                if self.resolver.has_decl(n) {
                    self.resolver.redefine_decl(decl.clone());
                } else {
                    self.resolver.commit_decl(decl.clone());
                }
            } else {
                self.resolver.commit_decl(decl.clone());
            }
        }

        // Replace the runtime with the one carrying mock handlers.
        self.runtime = result.runtime;

        writeln!(output, "\x1b[1mimported from trace:\x1b[0m")?;
        for name in &result.tool_names {
            let call_count = mocks.tools.get(name).map(|c| c.len()).unwrap_or(0);
            writeln!(output, "  tool \x1b[36m{name}\x1b[0m ({call_count} recorded call(s))")?;
        }
        for name in &result.prompt_names {
            let call_count = mocks.prompts.get(name).map(|c| c.len()).unwrap_or(0);
            writeln!(output, "  prompt \x1b[35m{name}\x1b[0m ({call_count} recorded call(s))")?;
        }
        output.flush()
    }

    fn cmd_import_source<W: Write>(&mut self, rest: &str, output: &mut W) -> io::Result<()> {
        // Parse: `:import "path.cor"` or `:import name from "path.cor"`
        //        optionally with "watching" suffix
        let watching = rest.trim_end().ends_with(" watching")
            || rest.trim_end().ends_with("\" watching");
        let rest_clean = if watching {
            rest.trim_end()
                .strip_suffix(" watching")
                .unwrap_or(rest)
                .trim()
        } else {
            rest.trim()
        };

        let (target_name, path_str) = if let Some(idx) = rest_clean.find(" from ") {
            let name = rest_clean[..idx].trim().to_string();
            let path = rest_clean[idx + 6..].trim().trim_matches('"').to_string();
            (Some(name), path)
        } else {
            let path = rest_clean.trim().trim_matches('"').to_string();
            (None, path)
        };

        let path = std::path::Path::new(&path_str);
        let source = match source_import::parse_source(path) {
            Ok(s) => s,
            Err(e) => {
                writeln!(output, "{e}")?;
                output.flush()?;
                return Ok(());
            }
        };

        if !source.errors.is_empty() {
            writeln!(output, "resolution errors in `{}`:", path.display())?;
            for err in &source.errors {
                writeln!(output, "  {err}")?;
            }
            output.flush()?;
            return Ok(());
        }

        let import_result = if let Some(ref name) = target_name {
            match source_import::import_selective(&source, name) {
                Ok(r) => r,
                Err(e) => {
                    writeln!(output, "{e}")?;
                    output.flush()?;
                    return Ok(());
                }
            }
        } else {
            source_import::import_all(&source)
        };

        for decl in &import_result.decls {
            let name = corvid_resolve::decl_name(decl).map(|s| s.to_string());
            if let Some(ref n) = name {
                if self.resolver.has_decl(n) {
                    self.resolver.redefine_decl(decl.clone());
                } else {
                    self.resolver.commit_decl(decl.clone());
                }
            } else {
                self.resolver.commit_decl(decl.clone());
            }
        }

        writeln!(output, "\x1b[1mimported from `{}`:\x1b[0m", path.display())?;
        for name in &import_result.imported_names {
            writeln!(output, "  \x1b[32m{name}\x1b[0m")?;
        }
        if !import_result.dependency_names.is_empty() {
            writeln!(
                output,
                "  \x1b[2m+ {} dependencies: {}\x1b[0m",
                import_result.dependency_names.len(),
                import_result.dependency_names.join(", ")
            )?;
        }

        if watching {
            let watcher = self.watcher.get_or_insert_with(|| {
                file_watch::FileWatchManager::new()
                    .expect("failed to create file watcher")
            });
            match watcher.watch(path) {
                Ok(()) => {
                    writeln!(output, "  \x1b[34mwatching {} for changes\x1b[0m", path.display())?;
                }
                Err(e) => {
                    writeln!(output, "  \x1b[31mfailed to watch: {e}\x1b[0m")?;
                }
            }
        }

        output.flush()
    }

    fn check_hot_reload<W: Write>(&mut self, output: &mut W) -> io::Result<()> {
        let changes = match self.watcher.as_mut() {
            Some(w) => w.poll_changes(),
            None => return Ok(()),
        };

        for change in changes {
            let display_path = change.path.display().to_string();
            match source_import::parse_source(&change.path) {
                Ok(source) => {
                    if !source.errors.is_empty() {
                        writeln!(
                            output,
                            "\x1b[33m(hot-reload) `{}` has resolution errors; skipping\x1b[0m",
                            display_path
                        )?;
                        continue;
                    }

                    let result = source_import::import_all(&source);
                    let mut redefined = Vec::new();
                    for decl in &result.decls {
                        let name = corvid_resolve::decl_name(decl).map(|s| s.to_string());
                        if let Some(ref n) = name {
                            if self.resolver.has_decl(n) {
                                if let Some(redef) = self.resolver.redefine_decl(decl.clone()) {
                                    redefined.push(redef.name.clone());
                                }
                            } else {
                                self.resolver.commit_decl(decl.clone());
                                if let Some(n) = name {
                                    redefined.push(n);
                                }
                            }
                        }
                    }

                    if !redefined.is_empty() {
                        writeln!(
                            output,
                            "\x1b[34m(hot-reload)\x1b[0m `{}`: redefined {}",
                            display_path,
                            redefined.join(", ")
                        )?;
                    }
                }
                Err(e) => {
                    writeln!(
                        output,
                        "\x1b[33m(hot-reload) `{}`: {e}\x1b[0m",
                        display_path
                    )?;
                }
            }
        }
        output.flush()
    }

    fn cmd_locals<W: Write>(&self, output: &mut W) -> io::Result<()> {
        if self.locals.is_empty() {
            writeln!(output, "(no locals)")?;
        } else {
            for local in &self.locals {
                writeln!(output, "  {}: {} = {}",
                    local.name,
                    local.ty.display_name(),
                    render_value(&local.value),
                )?;
            }
        }
        output.flush()
    }

    fn commit_turn_locals(
        &mut self,
        ir: &IrFile,
        checked: &Checked,
        env: &Env,
        result_name: Option<&str>,
    ) -> Option<Value> {
        let agent = ir
            .agents
            .iter()
            .find(|agent| agent.name == REPL_AGENT_NAME)
            .expect("synthetic repl agent must exist");

        let mut committed = Vec::new();
        let mut seen_names = HashSet::new();

        for param in &agent.params {
            if let Some(value) = env.lookup(param.local_id) {
                seen_names.insert(param.name.clone());
                committed.push(StoredLocal {
                    name: param.name.clone(),
                    ty: param.ty.clone(),
                    value,
                });
            }
        }

        let result = collect_turn_locals(
            &agent.body,
            checked,
            env,
            result_name,
            &mut seen_names,
            &mut committed,
        );

        self.locals = committed;
        self.typer.commit_locals(
            self.locals
                .iter()
                .map(|local| ReplLocal {
                    name: local.name.clone(),
                    ty: local.ty.clone(),
                })
                .collect(),
        );
        result
    }

    fn write_resolve_errors<W: Write>(
        &self,
        turn: &ResolvedTurn,
        output: &mut W,
    ) -> io::Result<bool> {
        if turn.resolved.errors.is_empty() {
            return Ok(false);
        }
        self.write_errors(turn.resolved.errors.iter(), output)?;
        Ok(true)
    }

    fn write_type_errors<W: Write>(&self, checked: &Checked, output: &mut W) -> io::Result<bool> {
        if checked.errors.is_empty() {
            return Ok(false);
        }
        self.write_errors(checked.errors.iter(), output)?;
        Ok(true)
    }

    fn write_errors<W: Write, I, E>(&self, errors: I, output: &mut W) -> io::Result<()>
    where
        I: IntoIterator<Item = E>,
        E: std::fmt::Display,
    {
        for error in errors {
            writeln!(output, "{error}")?;
        }
        output.flush()
    }

    fn enter_replay<W: Write>(&mut self, path: &str, output: &mut W) -> io::Result<()> {
        match ReplaySession::load(path) {
            Ok(session) => {
                writeln!(output, "{}", session.summary_line())?;
                output.flush()?;
                self.replay = Some(ReplayState {
                    session,
                    current: None,
                    next: 0,
                });
            }
            Err(error) => {
                writeln!(output, "{error}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    fn step_replay<W: Write>(&mut self, count: usize, output: &mut W) -> io::Result<()> {
        let Some(replay) = self.replay.as_mut() else {
            writeln!(output, "replay mode is not active")?;
            output.flush()?;
            return Ok(());
        };

        if replay.next >= replay.session.steps.len() {
            writeln!(output, "end of replay ({})", replay.session.final_status)?;
            output.flush()?;
            return Ok(());
        }

        let total = replay.session.steps.len();
        for _ in 0..count.max(1) {
            if replay.next >= total {
                break;
            }
            let index = replay.next;
            let step = &replay.session.steps[index];
            writeln!(output, "[step {}/{}] {}", index + 1, total, step.title())?;
            writeln!(output, "{}", step.render())?;
            replay.current = Some(index);
            replay.next += 1;
        }
        if replay.next >= total {
            writeln!(output, "end of replay ({})", replay.session.final_status)?;
        }
        output.flush()
    }

    fn run_replay<W: Write>(&mut self, output: &mut W) -> io::Result<()> {
        let remaining = self
            .replay
            .as_ref()
            .map(|replay| replay.session.steps.len().saturating_sub(replay.next))
            .unwrap_or(0);
        self.step_replay(remaining.max(1), output)
    }

    fn show_replay<W: Write>(&mut self, output: &mut W) -> io::Result<()> {
        let Some(replay) = self.replay.as_ref() else {
            writeln!(output, "replay mode is not active")?;
            output.flush()?;
            return Ok(());
        };
        match replay.current.and_then(|index| replay.session.steps.get(index)) {
            Some(step) => {
                writeln!(output, "{}", step.render())?;
            }
            None => {
                writeln!(output, "replay is loaded but no step has been shown yet")?;
            }
        }
        output.flush()
    }

    fn where_replay<W: Write>(&mut self, output: &mut W) -> io::Result<()> {
        let Some(replay) = self.replay.as_ref() else {
            writeln!(output, "replay mode is not active")?;
            output.flush()?;
            return Ok(());
        };
        let current = replay.current.map(|index| index + 1).unwrap_or(0);
        writeln!(
            output,
            "replay position: {}/{}",
            current,
            replay.session.steps.len()
        )?;
        output.flush()
    }

    fn quit_replay<W: Write>(&mut self, output: &mut W) -> io::Result<()> {
        if self.replay.take().is_some() {
            writeln!(output, "left replay mode")?;
        } else {
            writeln!(output, "replay mode is not active")?;
        }
        output.flush()
    }
}

fn display_type_rich(ty: &Type, symbols: &corvid_resolve::SymbolTable) -> String {
    match ty {
        Type::Struct(def_id) => {
            let entry = symbols.get(*def_id);
            entry.name.clone()
        }
        other => other.display_name(),
    }
}

fn format_typeref(ty: &corvid_ast::TypeRef) -> String {
    match ty {
        corvid_ast::TypeRef::Named { name, .. } => name.name.clone(),
        corvid_ast::TypeRef::Qualified { alias, name, .. } => {
            format!("{}.{}", alias.name, name.name)
        }
        corvid_ast::TypeRef::Generic { name, args, .. } => {
            let inner: Vec<String> = args.iter().map(format_typeref).collect();
            format!("{}<{}>", name.name, inner.join(", "))
        }
        corvid_ast::TypeRef::Weak { inner, .. } => {
            format!("Weak<{}>", format_typeref(inner))
        }
        corvid_ast::TypeRef::Function { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(format_typeref).collect();
            format!("({}) -> {}", ps.join(", "), format_typeref(ret))
        }
    }
}

fn collect_turn_locals(
    block: &IrBlock,
    checked: &Checked,
    env: &Env,
    result_name: Option<&str>,
    seen_names: &mut HashSet<String>,
    committed: &mut Vec<StoredLocal>,
) -> Option<Value> {
    let mut result = None;

    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let {
                local_id,
                name,
                ty,
                ..
            } => {
                if Some(name.as_str()) == result_name {
                    result = env.lookup(*local_id);
                    continue;
                }
                if seen_names.insert(name.clone()) {
                    if let Some(value) = env.lookup(*local_id) {
                        committed.push(StoredLocal {
                            name: name.clone(),
                            ty: ty.clone(),
                            value,
                        });
                    }
                }
            }
            IrStmt::If {
                then_block,
                else_block,
                ..
            } => {
                result = result.or_else(|| {
                    collect_turn_locals(
                        then_block,
                        checked,
                        env,
                        result_name,
                        seen_names,
                        committed,
                    )
                });
                if let Some(block) = else_block {
                    result = result.or_else(|| {
                        collect_turn_locals(
                            block,
                            checked,
                            env,
                            result_name,
                            seen_names,
                            committed,
                        )
                    });
                }
            }
            IrStmt::For {
                var_local,
                var_name,
                body,
                ..
            } => {
                if Some(var_name.as_str()) != result_name && seen_names.insert(var_name.clone()) {
                    if let Some(value) = env.lookup(*var_local) {
                        let ty = checked
                            .local_types
                            .get(var_local)
                            .cloned()
                            .unwrap_or(Type::Unknown);
                        committed.push(StoredLocal {
                            name: var_name.clone(),
                            ty,
                            value,
                        });
                    }
                }
                result = result.or_else(|| {
                    collect_turn_locals(
                        body,
                        checked,
                        env,
                        result_name,
                        seen_names,
                        committed,
                    )
                });
            }
            _ => {}
        }
    }

    result
}

fn history_path() -> io::Result<PathBuf> {
    let path = if cfg!(windows) {
        let appdata = std::env::var_os("APPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is not set"))?;
        history_path_from_vars(Some(PathBuf::from(appdata)), None, None)
    } else if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        history_path_from_vars(None, Some(PathBuf::from(data_home)), None)
    } else {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        history_path_from_vars(None, None, Some(PathBuf::from(home)))
    };
    Ok(path)
}

fn history_path_from_vars(
    appdata: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> PathBuf {
    if let Some(appdata) = appdata {
        return appdata.join("corvid").join(HISTORY_FILE);
    }
    if let Some(data_home) = xdg_data_home {
        return data_home.join("corvid").join(HISTORY_FILE);
    }
    home.expect("home must be present")
        .join(".local")
        .join("share")
        .join("corvid")
        .join(HISTORY_FILE)
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Repl;
    use super::{ensure_parent_dir, history_path_from_vars};
    use std::io::Cursor;

    #[test]
    fn persists_values_across_turns() {
        let input = Cursor::new(
            "\
type Point:\n\
    x: Int\n\
    y: Int\n\
\n\
p = Point(1, 2)\n\
p.x\n",
        );
        let mut output = Vec::new();
        Repl::run(input, &mut output).expect("repl run succeeds");
        let text = String::from_utf8(output).expect("valid utf8");
        assert!(text.contains("2"), "missing expression result in: {text}");
    }

    #[test]
    fn prints_result_values_with_type_aware_display() {
        let input = Cursor::new("Ok(Some(\"hi\"))\n");
        let mut output = Vec::new();
        Repl::run(input, &mut output).expect("repl run succeeds");
        let text = String::from_utf8(output).expect("valid utf8");
        assert!(text.contains("Ok(Some(\"hi\"))"), "unexpected output: {text}");
    }

    #[test]
    fn history_path_prefers_xdg_on_unix() {
        if cfg!(windows) {
            return;
        }
        let path = history_path_from_vars(
            None,
            Some("C:/tmp/xdg-home".into()),
            Some("C:/tmp/home".into()),
        );
        assert!(path.ends_with("corvid/history"), "unexpected history path: {}", path.display());
    }

    #[test]
    fn ensure_parent_dir_creates_directory_tree() {
        let temp = std::env::temp_dir()
            .join(format!("corvid-repl-test-{}", std::process::id()))
            .join("nested")
            .join("history");
        if let Some(parent) = temp.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
        ensure_parent_dir(&temp).expect("create parent dir");
        assert!(temp.parent().expect("parent").exists());
        if let Some(parent) = temp.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn cost_command_renders_cost_tree() {
        let mut repl = Repl::new().expect("repl init");
        let mut output = Vec::new();
        repl.process_source(
            "effect search_effect:\n    cost: $0.001\n    tokens: 12\n    latency_ms: 100\n",
            &mut output,
            false,
        )
        .expect("effect decl");
        repl.process_source(
            "effect plan_effect:\n    cost: $0.030\n    tokens: 835\n    latency_ms: 1100\n",
            &mut output,
            false,
        )
        .expect("effect decl");
        repl.process_source(
            "tool search(query: String) -> String uses search_effect",
            &mut output,
            false,
        )
        .expect("tool decl");
        repl.process_source(
            "prompt generate_plan(results: String) -> String uses plan_effect:\n    \"Plan.\"\n",
            &mut output,
            false,
        )
        .expect("prompt decl");
        repl.process_source(
            "@budget($1.00, tokens: 10000, latency: 5s)\nagent planner(query: String) -> String:\n    results = search(query)\n    plan = generate_plan(results)\n    return plan\n",
            &mut output,
            false,
        )
        .expect("agent decl");
        repl.process_source(":cost planner", &mut output, false)
            .expect("cost command");
        let text = String::from_utf8(output).expect("valid utf8");
        assert!(text.contains("planner"), "unexpected output: {text}");
        assert!(text.contains("search"), "unexpected output: {text}");
        assert!(text.contains("generate_plan"), "unexpected output: {text}");
        assert!(text.contains("tokens"), "unexpected output: {text}");
    }
}
