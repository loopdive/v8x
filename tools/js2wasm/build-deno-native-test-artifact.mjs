// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
// Development fixture only. Production packaging still requires clean pinned
// detached checkouts through build-deno-core-artifact.mjs.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { DENO_INPUTS, runtimeCompileOptions, createDenoSourceGraph } from "./build-deno-core-artifact.mjs";

const [compilerPath, fixturePath, outputPath] = process.argv.slice(2);
if (!compilerPath || !fixturePath || !outputPath) {
  throw new Error("usage: build-deno-native-test-artifact.mjs JS2_CHECKOUT PINNED_FIXTURES OUTPUT_WASM");
}
const js2 = resolve(compilerPath);
const denoSources = new Map();
const lockSources = [];
for (const input of DENO_INPUTS) {
  const raw = readFileSync(join(resolve(fixturePath), input.path));
  const sha256 = createHash("sha256").update(raw).digest("hex");
  assert.equal(raw.length, input.bytes, input.path + " byte count");
  assert.equal(sha256, input.sha256, input.path + " pinned source identity");
  denoSources.set(input.path, raw.toString("utf8"));
  lockSources.push({ path: input.path, bytes: raw.length, sha256 });
}
const { files, appRoot, graphInputs } = await createDenoSourceGraph({
  js2, profile: "runtime", execution: "aot", denoSources, lockSources,
});
const { compileMulti } = await import(pathToFileURL(join(js2, "src/index.ts")).href);
const result = await compileMulti(files, `${appRoot}/entry.ts`, {
  ...runtimeCompileOptions("aot"),
});
assert.equal(result.success, true, JSON.stringify(result.errors.filter(error => error.severity !== "warning")));
const module = new WebAssembly.Module(result.binary);
const imports = WebAssembly.Module.imports(module);
assert.deepEqual(imports.filter(item => item.module !== "v8x:deno" || item.kind !== "function"), []);
assert(WebAssembly.Module.exports(module).some(item => item.name === "__v8x_deno_core_namespace_handle"));
writeFileSync(resolve(outputPath), result.binary);
writeFileSync(resolve(outputPath) + ".test-inputs.json", JSON.stringify({
  kind: "local-native-test-fixture-not-production-package", compiler: js2,
  bytes: result.binary.byteLength, sources: graphInputs, imports,
}, null, 2) + "\n");
console.log(JSON.stringify({ bytes: result.binary.byteLength, sources: lockSources.length, imports: imports.length }));
