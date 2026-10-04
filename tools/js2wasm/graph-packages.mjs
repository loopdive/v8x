// Trusted build-side packaging. Deployment only reads source-bound native code.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, renameSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

export function graphDigest(entry, modules) {
  const hash = createHash("sha256").update("v8x/js2wasm graph binding v1\0");
  const append = bytes => {
    const size = Buffer.alloc(8);
    size.writeBigUInt64LE(BigInt(bytes.length));
    hash.update(size).update(bytes);
  };
  append(Buffer.from(entry));
  const count = Buffer.alloc(8);
  count.writeBigUInt64LE(BigInt(modules.length));
  append(count);
  for (const { specifier, source } of modules) {
    append(Buffer.from(specifier));
    append(Buffer.from(source));
  }
  return hash.digest("hex");
}

export function packageGraph(compiler, precompiler, entry, modules, directory, options = {}) {
  assert(modules.length > 0);
  assert.equal(new Set(modules.map(m => m.specifier)).size, modules.length);
  assert(modules.some(m => m.specifier === entry));
  mkdirSync(directory, {recursive:true});
  const staging = mkdtempSync(join(resolve(directory), ".graph-package-"));
  const manifest = join(staging, "modules.tsv");
  writeFileSync(manifest, modules.map((m, i) => {
    assert(!/[\t\r\n]/.test(m.specifier));
    const path = join(staging, `source-${i}.js`);
    writeFileSync(path, m.source);
    return `${m.specifier}\t${path}\n`;
  }).join(""));
  const raw = join(staging, "graph.wasm");
  const optimized = join(staging, "graph.opt.wasm");
  const native = join(staging, "graph.cwasm");
  const run = (program, args, extra = {}) => {
    const result = spawnSync(program, args, {encoding:"utf8", ...extra});
    assert.equal(result.status, 0, result.error?.message ?? result.stdout + result.stderr);
    return result;
  };
  run(process.execPath, ["--experimental-wasm-exnref", "--import", "tsx",
    join(resolve(compiler), "examples/v8x-js2wasm-spike/compile-graph.ts"),
    "--manifest", manifest, "--entry", entry, "--output", raw,
    ...(options.lifecycle ? ["--module-lifecycle", "true"] : [])], {cwd:resolve(compiler)});
  const optimizer = join(resolve(compiler), "node_modules/binaryen/bin/wasm-opt");
  run(process.execPath, [optimizer, raw, "--no-inline", "-O3",
    "--pass-arg=no-inline@__new_*", "--all-features", "--disable-custom-descriptors", "-g", "-o", optimized]);
  const result = run(resolve(precompiler), ["--exact", "precompiles_exact_deno_core_artifact", "--nocapture"], {
    env:{...process.env, V8X_JS2WASM_DENO_CORE_WASM:optimized,
      V8X_JS2WASM_DENO_CORE_AOT_OUTPUT:native,
      V8X_JS2WASM_DENO_CORE_AOT_ATTESTATION:join(staging,"attestation.json")},
  });
  assert.match(result.stdout, /1 passed; 0 failed/);
  const digest = graphDigest(entry, modules);
  const bytes = readFileSync(native);
  const target = join(resolve(directory), digest + ".cwasm");
  const sha = bytes => createHash("sha256").update(bytes).digest("hex");
  writeFileSync(join(staging,"binding"), `graph-sha256 ${digest}\nartifact-sha256 ${sha(bytes)}\n`);
  writeFileSync(join(staging,"inventory.json"), JSON.stringify({entry, modules,
    nativeSha256:sha(bytes), wasmSha256:sha(readFileSync(optimized))}, null, 2));
  renameSync(native, target);
  renameSync(join(staging,"inventory.json"), target+".json");
  renameSync(join(staging,"binding"), target+".graph-sha256");
  return target;
}
