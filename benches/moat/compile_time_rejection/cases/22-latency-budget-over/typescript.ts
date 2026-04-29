function slow_lookup(q: string): string {
  return q;
}

export function fast_path(q: string, budgetMs = 500): string {
  // BUG: known latency > intended budget; tsc passes.
  return slow_lookup(q);
}
