function burner(x: string): string {
  return x;
}

function helper(x: string): string {
  return burner(x);
}

export function caller(x: string, budgetUsd = 0.10): string {
  // BUG: helper cost > caller budget; tsc passes.
  return helper(x);
}
