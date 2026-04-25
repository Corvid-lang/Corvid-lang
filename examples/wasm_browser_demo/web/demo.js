import { instantiate } from "../target/wasm/refund_gate.js";

const amountInput = document.querySelector("#amount");
const approveButton = document.querySelector("#approve");
const denyButton = document.querySelector("#deny");
const resultEl = document.querySelector("#result");
const statusEl = document.querySelector("#status");
const traceEl = document.querySelector("#trace");

let approveNext = true;
const trace = [];

function renderTrace() {
  traceEl.textContent = JSON.stringify(trace, null, 2);
}

function setStatus(message) {
  statusEl.textContent = message;
}

function riskScore(amount) {
  if (amount >= 100n) return 90n;
  if (amount >= 50n) return 78n;
  return 20n;
}

const host = {
  prompts: {
    refund_score(amount) {
      return riskScore(BigInt(amount));
    },
  },
  approvals: {
    IssueRefund(amount) {
      setStatus(`Approval requested for $${amount.toString()}. Decision: ${approveNext ? "approved" : "denied"}.`);
      return approveNext;
    },
  },
  tools: {
    issue_refund(amount) {
      return BigInt(amount);
    },
  },
};

const corvid = await instantiate(host, { trace });
renderTrace();
setStatus("WASM module loaded. Run the agent to record prompt, approval, tool, and run events.");

function runWithDecision(decision) {
  approveNext = decision;
  const amount = BigInt(Math.trunc(Number(amountInput.value || 0)));
  try {
    const result = corvid.review_refund(amount);
    resultEl.textContent = `$${result.toString()}`;
    setStatus(result === 0n ? "No dangerous action was taken." : "Refund completed through the approved dangerous tool.");
  } catch (error) {
    resultEl.textContent = "blocked";
    setStatus(`The WASM agent trapped after a denied approval: ${error.message ?? error}`);
  }
  renderTrace();
}

approveButton.addEventListener("click", () => runWithDecision(true));
denyButton.addEventListener("click", () => runWithDecision(false));
