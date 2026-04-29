function burner(x: string): string {
  return x;
}

export function over(x: string, budgetUsd = 0.05): string {
  // BUG: known cost > intended budget; tsc passes.
  return burner(x);
}
