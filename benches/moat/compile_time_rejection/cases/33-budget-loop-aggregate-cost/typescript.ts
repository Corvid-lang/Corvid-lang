function ask(q: string): string {
  return q;
}

export function process(q: string, budgetUsd = 0.05): string {
  // BUG: aggregate cost > budget; tsc passes.
  const a = ask(q);
  const b = ask(q);
  return a + b;
}
