declare function prompt(message?: string): string | null;

function inner(x: string): string {
  return x;
}

function middle(x: string): string {
  return prompt(x) ?? "";
}

export function outer(x: string): string {
  // BUG: outer transitively non-deterministic; tsc passes.
  return middle(inner(x));
}
