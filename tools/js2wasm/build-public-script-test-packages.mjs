import assert from "node:assert/strict";
import { packageScript } from "./script-packages.mjs";

const [compiler, precompiler, output] = process.argv.slice(2);
assert(output, "usage: build-public-script-test-packages.mjs JS2_CHECKOUT PACKAGING_TEST_BINARY OUTPUT_DIR");
for (const source of [
  "41;42;",
  "void 0;",
  "globalThis.completionSaved={marker:42};globalThis.completionSaved;",
  "globalThis.completionSaved;",
  "throw globalThis.completionSaved;",
  "throw undefined;",
  "throw 42;",
  'globalThis.nativeSavedError=new TypeError("boom");throw globalThis.nativeSavedError;',
  "globalThis.nativeSavedError;",
  '({name:"TypeError",message:"boom"})',
  "globalThis.shouldNotRun=1;42;",
]) {
  console.log(await packageScript(compiler, precompiler, "<anonymous>", source, output));
}
