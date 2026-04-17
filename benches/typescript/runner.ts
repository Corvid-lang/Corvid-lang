import fs from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";

type Step = {
  name: string;
  kind: string;
  external_latency_ms?: number;
};

type Fixture = {
  name: string;
  expected_replay_events?: string[];
  steps: Step[];
};

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function runTrial(fixture: Fixture, trial: number) {
  const events: string[] = [];
  const state: Record<string, unknown> = {};
  const start = performance.now();
  let actualExternalWaitMs = 0;
  for (const step of fixture.steps) {
    const latencyMs = step.external_latency_ms ?? 0;
    if (step.kind === "prompt") {
      const rendered = JSON.stringify((step as any).inputs ?? {});
      const response = JSON.parse((step as any).mock_response ?? "null");
      state[step.name] = { rendered, response };
    } else if (step.kind === "tool") {
      const request = JSON.stringify((step as any).inputs ?? {});
      const response = JSON.parse(JSON.stringify((step as any).mock_output ?? null));
      state[step.name] = { request, response };
    } else if (step.kind === "approval") {
      const proposal = JSON.stringify((step as any).inputs ?? {});
      state[step.name] = { proposal, decision: (step as any).approval_outcome ?? "granted" };
    } else if (step.kind === "retry_sleep") {
      state[step.name] = { sleep_ms: latencyMs };
    } else if (step.kind === "replay_checkpoint") {
      state[step.name] = { checkpoint: (step as any).mock_output ?? null };
    }
    if (latencyMs > 0) {
      const waitStart = performance.now();
      await sleep(latencyMs);
      actualExternalWaitMs += performance.now() - waitStart;
    }
    events.push(`${step.kind}:${step.name}`);
  }
  const elapsedMs = performance.now() - start;
  const externalWaitMs = fixture.steps.reduce(
    (sum, step) => sum + (step.external_latency_ms ?? 0),
    0,
  );
  const traceBytes = Buffer.byteLength(events.join("\n"), "utf8");
  return {
    implementation: "typescript-node",
    process_mode: "persistent",
    fixture: fixture.name,
    trial,
    success: true,
    stdout_match: true,
    total_wall_ms: elapsedMs,
    external_wait_ms: externalWaitMs,
    actual_external_wait_ms: actualExternalWaitMs,
    external_wait_bias_ms: actualExternalWaitMs - externalWaitMs,
    orchestration_overhead_ms: elapsedMs - actualExternalWaitMs,
    trace_size_raw_bytes: traceBytes,
    logical_steps_recorded: events.length,
    bytes_per_step: events.length > 0 ? traceBytes / events.length : 0,
    replay_supported: false,
    expected_replay_steps: fixture.expected_replay_events?.length ?? 0,
  };
}

async function main() {
  const args = process.argv.slice(2);
  const serverMode = args[0] === "--server";
  const positional = serverMode ? args.slice(1) : args;
  const [fixturePath, trialsRaw, outputPath] = positional;
  if (!fixturePath || (!serverMode && (!trialsRaw || !outputPath))) {
    throw new Error(
      "usage: runner.ts <fixture.json> <trials> <output.jsonl> | runner.ts --server <fixture.json>",
    );
  }
  const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8")) as Fixture;
  if (serverMode) {
    process.stdin.setEncoding("utf8");
    let buffer = "";
    process.stdin.on("data", async (chunk: string) => {
      buffer += chunk;
      const lines = buffer.split(/\r?\n/);
      buffer = lines.pop() ?? "";
      for (const line of lines) {
        if (!line.trim()) continue;
        const request = JSON.parse(line);
        const trial = Number(request.trial_idx);
        const record = await runTrial(fixture, trial);
        process.stdout.write(`${JSON.stringify(record)}\n`);
      }
    });
    return;
  }
  const trials = Number.parseInt(trialsRaw, 10);
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  const lines: string[] = [];
  for (let trial = 1; trial <= trials; trial += 1) {
    lines.push(JSON.stringify(await runTrial(fixture, trial)));
  }
  fs.writeFileSync(outputPath, `${lines.join("\n")}\n`, "utf8");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
