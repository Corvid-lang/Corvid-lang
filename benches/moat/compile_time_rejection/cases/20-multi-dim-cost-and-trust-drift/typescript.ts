function settle(): void {
  return;
}

export function bad(): void {
  // BUG: cumulative cost + trust both drift past the caller's intent.
  return settle();
}
