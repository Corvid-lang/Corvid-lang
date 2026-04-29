function a(x: string): string { return x; }
function b(x: string): string { return x; }
function c(x: string): string { return x; }

export function pipeline(x: string, budgetUsd = 0.50): string {
  // BUG: cumulative cost > budget.
  return c(b(a(x)));
}
