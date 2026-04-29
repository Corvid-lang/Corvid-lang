declare function prompt(message?: string): string | null;

export function ask_human(q: string): string {
  // BUG: prompt() not captured by replay; tsc passes.
  return prompt(q) ?? "";
}
