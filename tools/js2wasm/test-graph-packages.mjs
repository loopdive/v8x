import test from "node:test";
import assert from "node:assert/strict";
import { graphDigest, packageGraph } from "./graph-packages.mjs";

test("graph binding matches the package accepted by native Rust replay", () => {
  const entry = "file:///module-execution-probe.js";
  assert.equal(graphDigest(entry, [{specifier:entry,
    source:"globalThis.moduleExecutionProbe=42; export const answer=42;"}]),
  "e5ea1b29832d0bf7e3ebcc71a36e469d2cd56f3d3ada3811b0fabb843ebda00f");
});
test("graph bindings distinguish entry, exact source, UTF-8 and module order", () => {
  const first = {specifier:"ext:one/é.js",source:"export const n=1;"};
  const second = {specifier:"ext:two.js",source:"export const n=2;"};
  const hash = graphDigest(first.specifier, [first,second]);
  assert.notEqual(hash, graphDigest(second.specifier, [first,second]));
  assert.notEqual(hash, graphDigest(first.specifier, [second,first]));
  assert.notEqual(hash, graphDigest(first.specifier, [{...first,source:first.source+" "},second]));
});
test("empty, duplicate and entry-less graphs are refused before build actions", () => {
  const item = {specifier:"a",source:""};
  for (const [entry,modules] of [["a",[]],["a",[item,item]],["b",[item]]])
    assert.throws(() => packageGraph("unused", "unused", entry, modules, "unused"));
});
