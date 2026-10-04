// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createContext, SourceTextModule } from "node:vm";

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
});
