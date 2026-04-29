function helper(x: string): string {
  return x;
}

export function compute(x: string): string {
  // BUG: helper is not annotated pure; tsc passes.
  return helper(x);
}
