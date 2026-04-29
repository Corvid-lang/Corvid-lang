function external(x: string): string {
  return x;
}

export function compute(x: string): string {
  // BUG: nothing in tsc enforces purity.
  return external(x);
}
