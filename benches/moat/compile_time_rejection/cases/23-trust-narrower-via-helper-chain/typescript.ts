type TrustLevel = "autonomous" | "human_required";

function deep_op(x: string): string {
  return x;
}

function layer_one(x: string): string {
  return deep_op(x);
}

function layer_two(x: string): string {
  return layer_one(x);
}

export function layer_three(x: string, declaredTrust: TrustLevel = "autonomous"): string {
  // BUG: human_required surfaces 3 layers down; outer claims autonomous; tsc passes.
  return layer_two(x);
}
