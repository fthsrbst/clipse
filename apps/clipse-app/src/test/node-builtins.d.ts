/**
 * The slice of Node that two tests need, declared rather than installed.
 *
 * `@types/node` is deliberately absent from this project — `vite.config.ts`
 * carries a `@ts-expect-error` over its one use of `process` rather than pull
 * the package in. Reading a committed installer bitmap off disk needs three
 * functions, and three functions do not justify reversing that.
 *
 * Test-only. Nothing under `src/` that ships may import these.
 */

declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function readFileSync(path: string): Uint8Array;
}

declare module "node:path" {
  export function resolve(...parts: string[]): string;
  export function dirname(path: string): string;
}

declare module "node:url" {
  export function fileURLToPath(url: string | URL): string;
}
