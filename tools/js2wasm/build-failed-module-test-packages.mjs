// Trusted build-side packages for dependency lifecycle failure controls.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { packageGraph } from "./graph-packages.mjs";

const [compiler, precompiler, output] = process.argv.slice(2);
assert(output, "usage: build-failed-module-test-packages.mjs JS2 PRECOMPILER OUTPUT");
const fixture = name => ({
  specifier: `file:///failed-module/${name}.js`,
  source: readFileSync(fileURLToPath(new URL(`../../tests/fixtures/js2wasm-failed-module/${name}.js`, import.meta.url)), "utf8"),
});
const entry = fixture("entry");
console.log(packageGraph(compiler, precompiler, entry.specifier,
  [entry, fixture("prefix"), fixture("shared"), fixture("later")], output));
