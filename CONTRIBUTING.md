# Contributing to Corvid

This document is for the people building Corvid — today that's a team of two, tomorrow maybe more. Read it once end-to-end before your first commit. After that, revisit it when you're tempted to take a shortcut.

---

## Mission

**Make programming AI agents as safe and natural as programming anything else.**

Today, building an AI agent in Python + Pydantic AI means stringing together libraries that can't talk to each other about the things that matter: what an action costs, whether it's reversible, whether the model was confident, whether a human approved. Every safeguard is a convention, a decorator, a code review. None of it is enforced by the language.

Corvid is the language that enforces what matters. A dangerous action without approval doesn't run at 3 AM and damage real users — it doesn't compile.

---

## Vision

**A standalone, natively-fast, AI-native programming language, shipped as a single binary, trusted by teams building production agents.**

At v1.0 a user downloads `corvid`, writes `.cor` files, and runs them. No Python. No SDK soup. No runtime assembly. The compiler enforces safety; the runtime makes LLM calls, dispatches tools, handles approvals, and writes traces — all natively, in Rust, in one binary.

When someone asks "what are you building?" the one-sentence answer is:

> *A compiled language where the compiler refuses to let your agent make an irreversible mistake.*

That's the thing users will still be describing to each other in ten years.

---

## Why we're building Corvid

Three reasons. All three matter.

**1. The safety gap is real.**
Every week, another agent-gone-wrong story hits the news — auto-issued refunds, unintended emails, deleted accounts. These aren't bugs that would fail code review. They're bugs that *no review would catch* because the rules ("don't issue a refund without approval") live in human conventions, not in types. A compiler can catch them. Corvid does.

**2. AI is a first-class citizen nowhere.**
Every language we use — Python, TypeScript, Go, Rust — was designed before LLMs existed. In all of them, AI is a library. Prompts are strings. Model outputs are untyped. Non-determinism is invisible. That's wrong. AI is a distinct computational primitive, like async or ownership, and deserves the same language-level respect.

**3. We're competent enough to do it right, and stubborn enough to not ship until it is.**
Most language projects fail because founders lose faith in year 2. We've decided — explicitly, in writing — that we'll take as long as it takes. Users told us they don't care if it takes 10 years. We believe them.

---

## Values

Five principles govern every decision. When you're uncertain, come back here.

### 1. Hard way, no shortcuts.

**This is the most important rule.** Every shortcut in the compiler is a shortcut in the safety promise of the language. A language that enforces discipline cannot be built sloppily.

Concrete applications:

- **No `.unwrap()` or `.expect()` outside tests.** If a value *can* be `None`, handle it. If it *cannot*, document why with a comment.
- **No silent `Type::Unknown` when we could infer the real type.** Use `Unknown` only as a deliberate "graceful degradation" choice, not because inference was hard.
- **No skipped tests.** If you added a feature, you added tests for it. If a test is hard to write, that's the test that proves the code actually works.
- **No premature transpilation.** Where we can compile natively, we compile natively. Python transpile is a bootstrapping tactic, not a permanent answer.
- **No "just this once" feature additions.** If it's worth doing, it deserves a dev-log proposal and a pre-phase chat. If it's not worth that, it's not worth doing.
- **No commits you don't understand.** Reading code from a collaborator, stop at the first line you can't explain to yourself. Ask. No rubber-stamp reviews.

If you're about to write something and you think "this will do for now" — **stop**. It won't. Do it properly the first time. That's the contract.

### 2. Honesty over hype.

When code doesn't work, say so. When a decision is uncertain, say so. When you don't know something, say so. The dev log should read like a truthful lab notebook, not a marketing blog. If we write "users will love this!" in a PR description, someone should push back.

### 3. Discipline over speed.

Pre-phase chat before code. Dev-log entry after every session. Tests at every phase boundary. The cadence is non-negotiable. We've already watched other language projects die from "just skip the discussion this time." We don't.

### 4. Simplicity over cleverness.

If a function needs a comment to be understood, rewrite the function until it doesn't. If a type system needs a tutorial to use, simplify the type system. Rust lets us be very clever. That's a trap. Corvid's users should be able to read the compiler and understand it.

### 5. Readability over brevity.

For both Corvid code (what users write) and Rust code (what we write). Names are English words. Control flow is explicit. Comments explain *why*, not *what*. A file that fits on one screen is better than a file of the same features that compiles faster.

---

## How we work

### Phase-based development

Work happens in numbered phases. Every phase:

1. **Pre-phase chat.** Before any code, the person driving the phase writes a short brief: what it does, key concepts, decisions to make, scope, success criteria. The other reviews. Decisions get locked before we type `cargo new`.

2. **Build it.** Tests go in during the build, not after.

3. **Phase boundary.** Tests green. Dev-log entry written. A one-paragraph summary to the other developer explains what changed and why.

4. **Next phase gets its own chat.** No chaining. No "while I'm here, let me also…" Phase creep is how language projects die.

See [`ROADMAP.md`](./ROADMAP.md) for the phase plan and [`dev-log.md`](./dev-log.md) for past phase entries.

### The dev log

`dev-log.md` at the repo root. Append-only. One entry per working session per developer. Every entry has:

- **What changed**: file paths and the shape of the change.
- **Decisions made**: the fork you hit, the options, the pick, the reason.
- **Scope calls**: what you deferred, why.
- **Test delta**: count before → count after, green.

If you worked and the log didn't get an entry, the work is incomplete.

### Communication between developers

- Every non-trivial PR gets reviewed by the other developer. No self-merges.
- Reviews read for *correctness* first, *style* second. If the code is correct but ugly, comment and move on. If the code is wrong but pretty, block.
- Disagreements get resolved in the dev log, not in PR threads that disappear.

---

## Code rules

### Rust style

- **Edition 2021.** Rustfmt default. Clippy clean (we'll add a CI rule soon).
- **No `unsafe`** anywhere in the compiler or interpreter. The runtime may eventually need it for FFI, and that code will live in a clearly-named `unsafe/` module with extensive comments.
- **Error handling** is `Result<T, E>` with typed errors, not `anyhow::Error` inside crates. `anyhow` is fine at the CLI boundary only.
- **No `.clone()` out of laziness.** If you can take `&T` instead of `T`, do. If borrow-checker pain makes you want to clone, rethink the ownership first.
- **Public APIs are documented.** Every `pub fn`, `pub struct`, `pub enum` variant has a `///` doc comment explaining *what* and *why*. Private items document when the why is non-obvious.

### Error messages

Every `Error` type in the compiler carries:

- A **span** pointing at the offending source.
- A **message** describing what went wrong in one line.
- A **hint** suggesting how to fix it, whenever a fix is possible.

See `corvid-types/src/errors.rs` for the pattern. If you add a new error kind, give it a stable code (`E0xxx`) and update the rendering table in `corvid-driver/src/render.rs`.

### Tests

- Every phase ends with green tests.
- Unit tests live next to the code they test (`#[cfg(test)] mod tests { ... }`).
- End-to-end tests live in `tests/e2e/` and use real `.cor` fixtures.
- If a bug gets reported, the fix commits include a regression test.

Before you run `cargo commit` or equivalent: `cargo test --workspace` must be green. No exceptions.

### Invention shipping contract

Corvid-specific inventions must be public, runnable, and test-backed when they
ship. If a feature is important enough to make Corvid different from ordinary
languages and libraries, it is important enough to appear in the project front
door.

Every new invention ships with:

- A README catalog entry, or an explicit update explaining why an existing entry
  already covers it.
- A `corvid tour --topic <name>` demo whose source compiles through the normal
  driver pipeline.
- A `docs/inventions.md` proof-matrix row with shipped status, runnable command,
  test coverage, spec link, and explicit non-scope.
- A spec or reference-doc link that defines the behavior.
- Tests that validate the behavior named in the catalog entry.

No hidden inventions. No prose-only inventions. No launch claims without a
runnable command and a test path.

### Scope

The feature roadmap lives in [`FEATURES.md`](./FEATURES.md). Before adding a new feature:

1. Is it on the roadmap? If yes, is it in the current milestone?
2. If it's not on the roadmap, open a dev-log entry titled `feature-proposal: <name>` with pain / smallest version / which milestone.
3. Default answer is **no**. Scope discipline is the single most important factor in whether Corvid ships.

### Corvid source style

When we write `.cor` examples or stdlib:

- `snake_case` for values, functions, tools, agents.
- `PascalCase` for user-declared types.
- One declaration per logical group; leave blank lines between groups.
- Prompt bodies in triple-quoted strings when they span lines.

---

## Practical

### First-time setup

```bash
git clone <repo>
cd corvid
cargo build --workspace
cargo test --workspace                 # 134+ tests should pass
pip install -e './runtime/python[dev]' # only needed for --target=python work
```

### Run the offline demo

```bash
cd examples/refund_bot_demo
cargo run --manifest-path ../../Cargo.toml --bin corvid -- build src/refund_bot.cor
python3 tools.py
```

### Run the test suite

```bash
cargo test --workspace
cd runtime/python && python3 -m pytest
```

### Git

- Branch off `main`. One PR per phase (or sub-phase).
- Commits: imperative subject lines; body explains *why* for non-trivial changes.
- Rebase, don't merge. Keep history linear.
- Squash merges by default. Preserve individual commits only when they're useful history.

### What to do when you're stuck

1. Re-read the relevant phase's dev-log entry.
2. Re-read the spec sections in `ARCHITECTURE.md`.
3. Write down what you're trying to do in one sentence. If you can't, the problem is that you don't know yet — step back and think.
4. Ask the other developer. Don't spin alone for more than a day.

### What to do when you disagree

Disagreements about design are good. Resolve them by:

1. Each side writes 3–5 sentences stating their position in the dev log.
2. The other responds.
3. If still unresolved after one round, the person driving the current phase decides, documents the decision, and we move on. Future phases can revisit.

Never block a PR for more than 24 hours on a disagreement without writing it up.

---

## Reminder

Every decision we make has exactly one question behind it:

> *If a 10-year-old user of Corvid — a founder who's betting her company on it — opens up the code and reads this, will she trust us?*

If yes, ship it. If no, fix it first. That's the job.
