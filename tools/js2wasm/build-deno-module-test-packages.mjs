// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { packageGraph } from "./graph-packages.mjs";
import { packageScript } from "./script-packages.mjs";
import { denoModuleFixtures } from "./deno-module-fixtures.mjs";

const [compiler, precompiler, deno, output] = process.argv.slice(2);
assert(output, "usage: build-deno-module-test-packages.mjs JS2 PRECOMPILER DENO OUTPUT");
assert.notEqual(process.platform, "win32", "filename fixture selects the original Unix arm");
const pin = "1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44";
const source = execFileSync("git", ["-C", deno, "show", `${pin}:libs/core/modules/tests.rs`], { encoding: "utf8" });
const { graphs, scripts } = denoModuleFixtures(source);
const core = execFileSync("git", ["-C", deno, "show", `${pin}:libs/core/mod.js`], { encoding: "utf8" });
for (const { name, entry, source } of graphs) {
  const modules = [{ specifier: entry, source }];
  if (name === "builtin_core_module") modules.push({ specifier: "ext:core/mod.js", source: core });
  console.log(name, packageGraph(compiler, precompiler, entry, modules, join(output, "graphs")));
}
for (const { test, specifier, source } of scripts) {
  console.log(test, specifier, await packageScript(compiler, precompiler, specifier, source, join(output, "scripts")));
}
