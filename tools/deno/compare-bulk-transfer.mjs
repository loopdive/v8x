// Same unchanged Deno workload, alternating preserved scalar and bulk bundles.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { performance } from "node:perf_hooks";
const [out, scalar, bulk, scalarCore, bulkCore, provider, candidateProvider = provider] = process.argv.slice(2);
if (!provider) throw Error("usage: compare-bulk-transfer.mjs OUT SCALAR_EXE BULK_EXE SCALAR_CORE BULK_CORE PROVIDER [CANDIDATE_PROVIDER]");
mkdirSync(out, {recursive:true});
const expected = readFileSync(new URL("./js2wasm-poc-expected.stdout", import.meta.url), "utf8");
const rows = [];
for (let round = 0; round < 5; round++) {
  for (const name of round % 2 ? ["bulk", "scalar"] : ["scalar", "bulk"]) {
    const env = {...process.env};
    for (const k of Object.keys(env)) if (k.startsWith("V8X_JS2WASM_")) delete env[k];
    env.V8X_JS2WASM_DENO_CORE_AOT_MODULE = name === "bulk" ? bulkCore : scalarCore;
    const selectedProvider = name === "bulk" ? candidateProvider : provider;
    if (selectedProvider !== "none") env.V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE = selectedProvider;
    const start = performance.now();
    const result = spawnSync("/usr/bin/time", ["-l", name === "bulk" ? bulk : scalar], {env, encoding:"utf8"});
    const row = {name, round, ms:performance.now()-start, status:result.status, stdout:result.stdout, stderr:result.stderr};
    rows.push(row);
    writeFileSync(out + "/results.json", JSON.stringify({inputs:{scalar,bulk,scalarCore,bulkCore,provider,candidateProvider},rows},null,2));
    if (result.status !== 0 || result.stdout !== expected) throw Error("incorrect " + name + ": " + result.stderr);
    console.log(name, round+1, row.ms.toFixed(1), "ms PASS");
  }
}
for (const name of ["scalar", "bulk"]) {
  const values=rows.filter(r=>r.name===name).map(r=>r.ms).sort((a,b)=>a-b);
  console.log(name, "median", values[2].toFixed(1), "ms");
}
