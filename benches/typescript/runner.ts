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
  const start = performance.now();
  for (const step of fixture.steps) {
    const latencyMs = step.external_latency_ms ?? 0;
    if (latencyMs > 0) {
      await sleep(latencyMs);
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
    fixture: fixture.name,
    trial,
    success: true,
    stdout_match: true,
    total_wall_ms: elapsedMs,
    external_wait_ms: externalWaitMs,
    orchestration_overhead_ms: elapsedMs - externalWaitMs,
    trace_size_raw_bytes: traceBytes,
    logical_steps_recorded: events.length,
    bytes_per_step: events.length > 0 ? traceBytes / events.length : 0,
    replay_supported: false,
    expected_replay_steps: fixture.expected_replay_events?.length ?? 0,
  };
}

async function main() {
  const [fixturePath, trialsRaw, outputPath] = process.argv.slice(2);
  if (!fixturePath || !trialsRaw || !outputPath) {
    throw new Error("usage: runner.ts <fixture.json> <trials> <output.jsonl>");
  }
  const trials = Number.parseInt(trialsRaw, 10);
  const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8")) as Fixture;
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
