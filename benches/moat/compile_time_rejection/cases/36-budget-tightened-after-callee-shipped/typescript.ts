function work(x: string): string {
  return x;
}

export function op(x: string, budgetUsd = 0.20): string {
  // BUG: tighter budget passes tsc; runtime overrun ships.
  return work(x);
}
