function talkative(q: string): string {
  return q;
}

export function quiet(q: string, budgetTokens = 2000): string {
  // BUG: known token cost > intended budget; tsc passes.
  return talkative(q);
}
