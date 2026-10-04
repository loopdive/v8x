import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { packageFunction } from "./compiled-function-packages.mjs";

const [compiler, precompiler, wrapper, deno, output] = process.argv.slice(2);
assert(output, "usage: build-deno-lazy-function-packages.mjs JS2 PACKAGER DENO_WRAPPER DENO_CHECKOUT OUTPUT");
const records = [];
for (const name of ["lazy_script.js", "lazy_script_dep.js"]) {
  const input = join(deno, "libs/core/modules/testdata", name);
  const original = readFileSync(input);
  const wrapped = spawnSync(wrapper, [input], {encoding: "utf8"});
  assert.equal(wrapped.status, 0, wrapped.stderr);
  const specifier = `ext:test_ext/${name}`;
  const body = wrapped.stdout;
  const artifact = await packageFunction(compiler, precompiler, specifier, body, ["__bootstrap"], output);
  records.push({input, specifier, originalSha256: createHash("sha256").update(original).digest("hex"),
    body, parameters: ["__bootstrap"], artifact});
  console.log(artifact);
}
writeFileSync(join(output, "deno-lazy-function-inputs.json"), JSON.stringify({
  wrapper, deno, compiler, records,
}, null, 2) + "\n");
