declare function prompt(message?: string): string | null;

export function answer(q: string): string {
  // BUG: prompt() is non-deterministic; tsc passes.
  return prompt(q) ?? "";
}
