import assert from "node:assert/strict";
import { packageScript } from "./script-packages.mjs";
import { packageFunction } from "./compiled-function-packages.mjs";

const [compiler, precompiler, output] = process.argv.slice(2);
assert(output, "usage: build-reentrant-script-test-packages.mjs JS2_CHECKOUT PACKAGING_TEST_BINARY OUTPUT_DIR");
for (const source of [
  "41;42;",
  "throw globalThis.completionSaved;",
  "throw undefined;",
  "globalThis.reentrantScriptHost(0);",
  "globalThis.reentrantScriptHost(1);",
  "globalThis.reentrantScriptHost(2);",
  "globalThis.reentrantScriptHost(3);",
]) {
  console.log(await packageScript(compiler, precompiler, "<anonymous>", source, output));
}
for (const body of [
  "return value + 1;",
  "globalThis.functionBodyRuns = 1; return value + 1;",
  "throw value;",
]) {
  console.log(await packageFunction(compiler, precompiler, "<anonymous>", body, ["value"], output));
}
