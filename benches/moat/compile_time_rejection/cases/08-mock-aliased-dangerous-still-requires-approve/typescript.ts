import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function issue_refund(id: string, approval?: Approval): number {
  if (!approval) throw new Error("issue_refund requires approval");
  return 42;
}

// jest-style mock; tsc has no concept of "this mock preserves the
// dangerous marker." Tests use it without an approval token.
function fake_issue_refund(id: string, approval?: Approval): number {
  return 42;
}

export function test_unsafe_call(): void {
  // BUG: mocked function preserves no safety contract.
  const value = fake_issue_refund("o-1");
  if (value !== 42) throw new Error("test failed");
}
