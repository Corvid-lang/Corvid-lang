// Phase 23-F headless-Chromium browser smoke test for the WASM
// approval demo. The test exercises:
//
//   1. The generated ES loader (`refund_gate.js`) instantiates the
//      WASM module against typed prompt/tool/approval host capabilities
//      supplied from JS.
//   2. Approving a dangerous action runs the tool and surfaces a
//      non-zero result.
//   3. Denying a dangerous action blocks the tool and the page reports
//      the trapped agent.
//   4. The trace panel records schema-v2 events including
//      `run_started`, `approval_request`, `approval_decision`,
//      `tool_call`, `tool_result`, and `run_completed` — proving the
//      generated loader and the runtime trace contract still agree
//      after any change to either side.
//
// Without this CI job, a JS-loader regression would only be caught at
// launch rehearsal. The slice 23-F gate runs it on every push.

import { test, expect } from "@playwright/test";

test.describe("WASM browser approval demo", () => {
  test("approves and denies dangerous refund through typed host capabilities", async ({ page }) => {
    const consoleErrors = [];
    page.on("pageerror", (err) => consoleErrors.push(`pageerror: ${err.message}`));
    page.on("console", (msg) => {
      if (msg.type() === "error") {
        consoleErrors.push(`console.error: ${msg.text()}`);
      }
    });

    await page.goto("/web/");

    // --- Wait for the WASM module to instantiate -----------------
    await expect(page.locator("#status")).toContainText(
      "WASM module loaded.",
      { timeout: 30_000 }
    );

    // --- Approve path: should run the tool and show a non-zero result ---
    await page.locator("#amount").fill("120");
    await page.locator("#approve").click();

    await expect(page.locator("#result")).not.toHaveText("not run");
    await expect(page.locator("#status")).toContainText(/Approval requested for \$120/);
    await expect(page.locator("#status")).toContainText(/Decision: approved/);

    let traceText = await page.locator("#trace").textContent();
    expect(traceText).toBeTruthy();
    let trace = JSON.parse(traceText);
    expect(Array.isArray(trace)).toBe(true);

    // Required schema-v2 event kinds the generated loader must emit
    // for a successful approve+execute path. If any one of these is
    // missing, the JS loader and the runtime trace contract have
    // drifted.
    const approveKinds = trace.map((event) => event.kind);
    for (const kind of [
      "schema_header",
      "run_started",
      "approval_request",
      "approval_decision",
      "tool_call",
      "tool_result",
      "run_completed",
    ]) {
      expect(approveKinds, `approve trace must contain ${kind}`).toContain(kind);
    }

    const approval = trace.find((event) => event.kind === "approval_decision");
    expect(approval).toBeDefined();
    expect(approval.approved).toBe(true);
    expect(approval.label).toBe("IssueRefund");

    // --- Deny path: tool must NOT fire; the page reports trap. ---
    await page.locator("#deny").click();

    await expect(page.locator("#result")).toHaveText(/blocked|0/);
    await expect(page.locator("#status")).toContainText(
      /Decision: denied|trapped after a denied approval/
    );

    traceText = await page.locator("#trace").textContent();
    trace = JSON.parse(traceText);
    const denyDecisions = trace
      .filter((event) => event.kind === "approval_decision")
      .map((event) => event.approved);
    expect(denyDecisions).toContain(false);

    // No console / page errors during the run.
    expect(consoleErrors).toEqual([]);
  });
});
