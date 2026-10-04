import { packageGraph } from "./graph-packages.mjs";
const [compiler, precompiler, output] = process.argv.slice(2);
if (!output) throw Error("usage: build-module-evaluation-test-packages.mjs JS2 PACKAGER OUTPUT");
for (const [entry, source] of [
  ["file:///main_module.js", "if (!import.meta.main) throw Error();"],
  ["file:///side_module.js", "if (import.meta.main) throw Error();"],
  ["file:///module-execution-probe.js", "globalThis.moduleExecutionProbe=42; export const answer=42;"],
]) {
  console.log(packageGraph(compiler, precompiler, entry, [{specifier:entry,source}], output));
}
