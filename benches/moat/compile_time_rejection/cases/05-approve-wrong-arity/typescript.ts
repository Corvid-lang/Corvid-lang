import { z } from "zod";

const Approval = z.object({ label: z.string(), args: z.array(z.string()) });
type Approval = z.infer<typeof Approval>;

function send_email(to: string, body: string, approval?: Approval): void {
  if (!approval || approval.args.length !== 2) {
    throw new Error("approval arity mismatch");
  }
}

export function notify(to: string): void {
  // BUG: arity 1 but tool takes 2; tsc passes.
  send_email(to, to, { label: "SendEmail", args: [to] });
}
