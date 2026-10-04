// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { packageGraph } from "./graph-packages.mjs";

const [compiler, precompiler, output] = process.argv.slice(2);
assert(output, "usage: build-typed-module-test-packages.mjs JS2 PRECOMPILER OUTPUT");
const fixture = name => ({
  specifier: `file:///typed-module/${name}.ts`,
  source: readFileSync(fileURLToPath(new URL(`../../tests/fixtures/js2wasm-typed-module/${name}.ts`, import.meta.url)), "utf8"),
});
for (const name of ["first", "second"]) {
  const entry = fixture(name);
  console.log(name, packageGraph(compiler, precompiler, entry.specifier, [entry, fixture("shared")], output, { lifecycle: true }));
}
