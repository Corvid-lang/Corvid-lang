import { z } from "zod";

const Approval = z.object({ label: z.string() });
type Approval = z.infer<typeof Approval>;

function send_email(to: string, body: string, approval?: Approval): void {
  if (!approval) throw new Error("send_email requires approval");
}

export function notify(flag: boolean, to: string): void {
  if (flag) {
    send_email(to, to, { label: "SendEmail" });
    return;
  }
  // BUG: unconditional fallback path has no runtime guard.
  send_email(to, to);
}
