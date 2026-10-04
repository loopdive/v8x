// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createContext, SourceTextModule } from "node:vm";

test("fresh same-URL source executes before a cached source rejection on V8", async () => {
  const context = createContext({});
  const module = name => new SourceTextModule(readFileSync(new URL(`../../tests/fixtures/js2wasm-failed-module/${name}.js`, import.meta.url), "utf8"), { context, identifier: `file:///failed-module/${name}.js` });
  const prefix = module("prefix"), shared = module("shared"), middle = module("middle"), later = module("later");
  const entry = module("entry");
  const first = new Map([["./prefix.js", prefix], ["./shared.js", shared], ["./middle.js", middle], ["./later.js", later]]);
  await entry.link(request => { assert(first.has(request)); return first.get(request); });
  // Node refuses linking to an already errored Module. Link both consumers
  // first, then evaluate them in order to exercise the cached failure path.
  const fresh = module("prefix"), untouched = module("later"), cached = module("cached-entry");
  const second = new Map([["./prefix.js", fresh], ["./shared.js", shared], ["./later.js", untouched]]);
  await cached.link(request => { assert(second.has(request)); return second.get(request); });
  await assert.rejects(entry.evaluate(), error => error === context.moduleThrownToken);
  const token = context.moduleThrownToken;
  assert.equal(context.prefixRuns, 1);
  await assert.rejects(cached.evaluate(), error => error === token);
  assert.equal(context.prefixRuns, 2);
  assert.equal(fresh.status, "evaluated");
  assert.equal(untouched.status, "linked");
  assert.equal(context.cachedEntryRuns, undefined);
  assert.equal(context.laterRuns, undefined);
  assert.equal(context.moduleThrownToken, token);
  await assert.rejects(cached.evaluate(), error => error === token);
  assert.equal(context.prefixRuns, 2);
});

test("shared fixture preserves single evaluation, namespace identity and live imports on V8", async () => {
  const context = createContext({});
  const module = name => new SourceTextModule(readFileSync(new URL(`../../tests/fixtures/js2wasm-shared-module/${name}.js`, import.meta.url), "utf8"), { context });
  const shared = module("shared");
  const resolve = request => {
    assert.equal(request, "./shared.js");
    return shared;
  };
  const first = module("first");
  await first.link(resolve);
  await first.evaluate();
  assert.equal(first.namespace.initial, 2);
  assert.equal(context.sharedModuleRuns, 1);
  const second = module("second");
  await second.link(resolve);
  await second.evaluate();
  assert.equal(context.sharedModuleRuns, 1);
  assert.equal(second.namespace.same, true);
  assert.equal(second.namespace.shared, shared.namespace);
  assert.equal(second.namespace.observed, 2);
  assert.equal(second.namespace.runs, 1);
  assert.equal(shared.namespace.bump(), 3);
  assert.equal(second.namespace.read(), 3);
  assert.equal(second.namespace.readNamed(), 3);
  assert.equal(second.namespace.mutateNamespace(), 4);
  assert.equal(second.namespace.mutateNamed(), 5);
  assert.equal(second.namespace.receiverNamespace(), shared.namespace);
  assert.equal(second.namespace.receiverNamed(), undefined);
  assert.equal(second.namespace.spreadNamespace(), 10);
  assert.equal(second.namespace.spreadNamed(), 15);
  assert.equal(second.namespace.spreadMixed(), 20);
  assert.equal(second.namespace.spreadNested(), 65);
  assert.equal(second.namespace.spreadInvalid(), 1);
  assert.equal(shared.namespace.count, 65);
  for (const [name, expected] of [["optionalNamed", 66], ["optionalMethod", 67], ["optionalComputed", 68], ["optionalNamespace", 69]]) {
    assert.equal(second.namespace[name](), expected);
  }
  for (const name of ["absentCall", "absentReceiver", "chainSkip", "computedSkip", "nullSkip", "optionalNamedReceiver"]) {
    assert.equal(second.namespace[name](), undefined);
  }
  assert.equal(second.namespace.optionalReceiver(), shared.namespace);
  assert.equal(second.namespace.parenthesizedReceiver(), shared.namespace);
  assert.equal(second.namespace.nestedReceiver(), shared.namespace.nested);
  assert.equal(second.namespace.parenBreak(), 1);
  assert.equal(shared.namespace.count, 69);
  assert.equal(second.namespace.nonCallable(), 1);
  assert.equal(shared.namespace.count, 70);
});
