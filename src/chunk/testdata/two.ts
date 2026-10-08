import { readFile } from "node:fs/promises";

// First.
export function one(x: number): number {
  const y = x + 1;
  return y * 2;
}

/** Second. */
export async function two(path: string): Promise<number> {
  const text = await readFile(path, "utf8");
  let total = 0;
  for (const line of text.split("\n")) {
    total += line.length;
  }
  return total;
}
