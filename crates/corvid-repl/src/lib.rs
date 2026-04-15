//! Interactive Corvid REPL.

mod replay;

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
use corvid_vm::{render_value, run_agent_with_env, Env, InterpError, InterpErrorKind, Value};
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

        let turn = self.resolver.resolve_decl_turn(decl.clone());
        if self.write_resolve_errors(&turn, output)? {
            return Ok(());
        }

        let checked = self.typer.typecheck_decl_turn(&turn.file, &turn.resolved);
        if self.write_type_errors(&checked, output)? {
            return Ok(());
        }

        self.resolver.commit_decl(decl);
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
        let args = self.locals.iter().map(|local| local.value.clone()).collect();
        let outcome = self.tokio.block_on(async {
            if allow_interrupt {
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
}
