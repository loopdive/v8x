// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
// Trusted build-side packaging only. Deployment never invokes this compiler.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, renameSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

export function scriptDigest(specifier, source) {
  const hash = createHash("sha256").update("v8x/js2wasm Script completion ABI v1\0");
  for (const text of [specifier, source]) {
    const bytes = Buffer.from(text, "utf8");
    const length = Buffer.alloc(8);
    length.writeBigUInt64LE(BigInt(bytes.length));
    hash.update(length).update(bytes);
  }
  return hash.digest("hex");
}

export function scriptCompileOptions(specifier) {
  return {
    // Resource names such as <anonymous> are not TypeScript virtual filenames.
    // The original resource specifier remains part of the package binding.
    target: "standalone", scriptGoal: true, allowJs: true, fileName: "script.ts",
    hostBridge: "always", deferTopLevelInit: true, standaloneScriptVarBindings: true,
    standaloneScriptLexicalImport: { module: "v8x:context", name: "__v8x_context_lexical" },
    standaloneScriptCompletionImport: { module: "v8x:context", name: "__v8x_context_script_completion" },
    standaloneAllocationOwnerExport: "localOwns",
    standaloneSymbolState: {module:"v8x:context"},
    standaloneScriptGetExport: "__v8x_script_get_export",
    standaloneScriptCallExport: "__v8x_script_call_export",
    standaloneScriptOwnNamesExport: "__v8x_script_own_names_export",
    standaloneScriptReflectionExports: { ownSymbols: "__v8x_script_own_symbols_export", descriptor: "__v8x_script_descriptor_export" },
    standaloneGlobalThisImport: { module: "v8x:context", name: "__v8x_context_global_this",
      owns: "__v8x_context_owns", get: "__v8x_context_get",
      arrayPrototype: "__v8x_context_array_prototype", exceptionTag: "__exn_tag" },
    link: ["v8x:context"],
  };
}

export function assertScriptABI(module) {
  const imports = WebAssembly.Module.imports(module);
  assert(imports.every(item => item.module === "v8x:context"), "AOT Script forbids non-Context imports");
  assert.equal(imports.filter(item => item.kind === "function" && item.name === "__v8x_context_script_completion").length,
    1, "AOT Script requires exactly one native completion sink");
  assert(WebAssembly.Module.exports(module).some(item => item.name === "__module_init" && item.kind === "function"),
    "AOT Script lacks native initializer");
}

export function assertOptimizedScriptImports(original, optimized) {
  const remaining = new Map();
  const key = (entry) => JSON.stringify([entry.module, entry.name, entry.kind]);
  for (const entry of original)
    remaining.set(key(entry), (remaining.get(key(entry)) ?? 0) + 1);
  for (const entry of optimized) {
    const identity = key(entry);
    const count = remaining.get(identity) ?? 0;
    assert(count > 0, `optimizer introduced native Script import ${identity}`);
    remaining.set(identity, count - 1);
  }
}

export async function packageScript(compilerPath, precompiler, specifier, source, outputPath) {
  const { compile } = await import(pathToFileURL(join(resolve(compilerPath), "src/index.ts")).href);
  const options = scriptCompileOptions(specifier);
  const result = await compile(source, options);
  assert.equal(result.success, true, JSON.stringify(result.errors));
  assertScriptABI(new WebAssembly.Module(result.binary));
  const output = resolve(outputPath);
  mkdirSync(output, { recursive: true });
  // Stage on the destination filesystem so publication can use atomic rename.
  const staging = mkdtempSync(join(output, ".script-package-"));
  const raw = join(staging, "script.wasm");
  const optimized = join(staging, "script.opt.wasm");
  const native = join(staging, "script.cwasm");
  const attestation = join(staging, "script.attestation.json");
  writeFileSync(raw, result.binary);
  const optimizer = join(resolve(compilerPath), "node_modules/binaryen/bin/wasm-opt");
  const version = spawnSync(process.execPath, [optimizer, "--version"], {encoding:"utf8"});
  assert.equal(version.status, 0, version.error?.message ?? version.stdout + version.stderr);
  const optimize = spawnSync(process.execPath, [optimizer, raw, "--no-inline", "-O3", "--pass-arg=no-inline@__new_*", "--all-features", "--disable-custom-descriptors", "-g", "-o", optimized], {encoding:"utf8"});
  assert.equal(optimize.status, 0, optimize.error?.message ?? optimize.stdout + optimize.stderr);
  const optimizedBytes = readFileSync(optimized);
  const optimizedModule = new WebAssembly.Module(optimizedBytes);
  assertScriptABI(optimizedModule);
  assertOptimizedScriptImports(WebAssembly.Module.imports(new WebAssembly.Module(result.binary)),
    WebAssembly.Module.imports(optimizedModule));
  const run = spawnSync(resolve(precompiler), ["--exact", "precompiles_exact_deno_core_artifact", "--nocapture"], {
    encoding: "utf8", env: { ...process.env, V8X_JS2WASM_DENO_CORE_WASM: optimized,
      V8X_JS2WASM_DENO_CORE_AOT_OUTPUT: native, V8X_JS2WASM_DENO_CORE_AOT_ATTESTATION: attestation },
  });
  assert.equal(run.status, 0, run.error?.message ?? run.stdout + run.stderr);
  assert.match(run.stdout, /1 passed; 0 failed/, "precompiler must execute one real packaging test");
  const bytes = readFileSync(native);
  const digest = scriptDigest(specifier, source);
  const nativeDigest = createHash("sha256").update(bytes).digest("hex");
  const target = join(output, digest + ".cwasm");
  // The binding is published last, after the exact verified native bytes.
  writeFileSync(join(staging, "binding"), `graph-sha256 ${digest}\nartifact-sha256 ${nativeDigest}\n`);
  writeFileSync(join(staging, "manifest"), JSON.stringify({ goal: "script", completionABI: 1,
    specifier, source, sourceSha256: createHash("sha256").update(source).digest("hex"),
    rawWasmSha256: createHash("sha256").update(result.binary).digest("hex"),
    wasmSha256: createHash("sha256").update(optimizedBytes).digest("hex"),
    optimizer: version.stdout.trim(), nativeSha256: nativeDigest, options }, null, 2) + "\n");
  renameSync(native, target);
  renameSync(join(staging, "manifest"), target + ".json");
  renameSync(join(staging, "binding"), target + ".graph-sha256");
  return target;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [compiler, precompiler, specifier, sourceFile, output] = process.argv.slice(2);
  assert(output, "usage: script-packages.mjs JS2_CHECKOUT PACKAGING_TEST_BINARY SPECIFIER SOURCE_FILE OUTPUT_DIR");
  console.log(await packageScript(compiler, precompiler, specifier, readFileSync(sourceFile, "utf8"), output));
}
