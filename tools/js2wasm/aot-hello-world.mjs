// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import { createHash } from "node:crypto";

// Closed-world program, not a general eval-to-function rewrite. The pinned
// program has undefined completion and no external observer of declarations.
// Repeated execution and other scripts are refused.
export function aotHelloWorldSource(source) {
  if (typeof source !== "string" || createHash("sha256").update(source).digest("hex") !==
      "33bf6b9698833319ad98c0cf88f2fb4dd7634859816ec784aa8902b3eeba1804") {
    throw new Error("unsupported closed-world AOT program: expected pinned Deno hello_world source");
  }
  return `
const expectedSource = ${JSON.stringify(source)};
let executed = false;
export function runAotHostScript(source: string): undefined {
  if (source !== expectedSource) throw new Error("closed-world AOT artifact rejects unknown script");
  if (executed) throw new Error("closed-world AOT program has already executed");
  executed = true;
  const Deno: any = (globalThis as any).Deno;
${source}
  return undefined;
}
`;
}
