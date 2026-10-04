import { packageGraph } from "./graph-packages.mjs";
const [compiler, precompiler, output] = process.argv.slice(2);
if (!output) throw Error("usage: build-import-meta-test-package.mjs JS2 PACKAGER OUTPUT");
const entry = "file:///import-meta-identity.js";
const source = 'export const meta=import.meta; export const custom=import.meta.custom; export const resolved=import.meta.resolve("./child.js"); export function read(){return import.meta;}';
console.log(packageGraph(compiler, precompiler, entry, [{specifier:entry,source}], output));
