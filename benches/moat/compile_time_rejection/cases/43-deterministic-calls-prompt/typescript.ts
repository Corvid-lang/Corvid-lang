function classify(x: string): string {
  return `label-for-${x}`;
}

export function compute(x: string): string {
  // BUG: prompt call non-deterministic; tsc passes.
  return classify(x);
}
