function burner(x: string): string {
  return x;
}

function helper(x: string): string {
  return burner(x);
}

export function outer(x: string): string {
  // BUG: helper's cumulative cost > intended outer budget; tsc passes.
  return helper(x);
}
