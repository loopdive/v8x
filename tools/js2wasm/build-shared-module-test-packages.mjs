// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { packageGraph } from "./graph-packages.mjs";

const [compiler, precompiler, output] = process.argv.slice(2);
assert(output, "usage: build-shared-module-test-packages.mjs JS2 PRECOMPILER OUTPUT");
const fixture = name => ({
  specifier: `file:///shared-module/${name}.js`,
  source: readFileSync(fileURLToPath(new URL(`../../tests/fixtures/js2wasm-shared-module/${name}.js`, import.meta.url)), "utf8"),
});
for (const name of ["first", "second"]) {
  const entry = fixture(name);
  console.log(name, packageGraph(compiler, precompiler, entry.specifier, [entry, fixture("shared")], output));
}
