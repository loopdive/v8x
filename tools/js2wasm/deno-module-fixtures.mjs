// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import assert from "node:assert/strict";

export function functionSource(source, name) {
  assert(/^[A-Za-z_]\w*$/.test(name), "invalid Rust test identifier");
  const matches = [...source.matchAll(new RegExp(`^(?:async )?fn ${name}\\(\\)`, "gm"))];
  assert.equal(matches.length, 1, `missing or ambiguous original Deno test ${name}`);
  const match = matches[0];
  const end = source.indexOf("\n#[", match.index);
  return source.slice(match.index, end < 0 ? undefined : end);
}

export function rawBinding(body, name) {
  const match = new RegExp(`let ${name} = r#"([\\s\\S]*?)"#;`).exec(body);
  assert(match, `missing original raw binding ${name}`);
  return match[1];
}

export function asciiLiterals(body) {
  return [...body.matchAll(/ascii_str!\(\s*(?:r#"([\s\S]*?)"#|("(?:[^"\\]|\\.)*"))\s*\)/g)]
    .map(match => match[1] ?? JSON.parse(match[2]));
}

export function rawScripts(body, test) {
  return [...body.matchAll(/\.execute_script\(\s*("(?:[^"\\]|\\.)*"),\s*r#"([\s\S]*?)"#,?\s*\)/g)]
    .map(match => ({ test, specifier: JSON.parse(match[1]), source: match[2] }));
}

// Preserve the exact original runtime source bytes, including whitespace.
// Fail on unrecognized literal layouts rather than guessing Rust semantics.
export function denoModuleFixtures(source, readFixture) {
  const graphs = [];
  const scripts = [];
  const resolve = functionSource(source, "import_meta_resolve");
  graphs.push({ name: "import_meta_resolve", entry: "file:///test.js", source: rawBinding(resolve, "source") });
  const filename = functionSource(source, "import_meta_filename_dirname");
  graphs.push({ name: "import_meta_filename_dirname", entry: "file:///main_module.js", source: rawBinding(filename, "code") });
  const builtin = functionSource(source, "builtin_core_module");
  graphs.push({ name: "builtin_core_module", entry: "ext:///main_module.js", source: rawBinding(builtin, "source_code") });
  for (const name of ["evaluate_already_evaluated_module", "evaluate_already_evaluated_module_sync"]) {
    const body = functionSource(source, name);
    const literal = /ascii_str!\(\s*("(?:[^"\\]|\\.)*")\s*\)/.exec(body);
    assert(literal, `missing original module literal in ${name}`);
    graphs.push({ name, entry: "file:///main.js", source: JSON.parse(literal[1]) });
    const checks = [...body.matchAll(/\.execute_script\(\s*"(check[12])",\s*("(?:[^"\\]|\\.)*")\s*\)/g)];
    assert.equal(checks.length, 2, `${name} must contain both original execution checks`);
    for (const check of checks) scripts.push({ test: name, specifier: check[1], source: JSON.parse(check[2]) });
  }
  const mainSide = asciiLiterals(functionSource(source, "main_and_side_module"));
  assert.equal(mainSide.length, 2, "main/side must contain both original module sources");
  for (const [index, name] of ["main", "side"].entries()) {
    graphs.push({ name: "main_and_side_module", entry: `file:///${name}_module.js`, source: mainSide[index] });
  }
  const mods = functionSource(source, "test_mods");
  const literals = asciiLiterals(mods);
  assert.equal(literals.length, 4, "test_mods must retain both specifier/source pairs");
  assert.equal(literals[0], "file:///a.js");
  assert.equal(literals[2], "file:///b.js");
  graphs.push({ name: "test_mods", entry: literals[0], source: literals[1],
    dependencies: [{ specifier: literals[2], source: literals[3] }] });
  const setup = rawScripts(mods, "test_mods");
  assert.equal(setup.length, 1, "test_mods must contain its original assertion setup Script");
  assert.equal(setup[0].specifier, "setup.js");
  scripts.push(...setup);
  const aliasTest = "test_lazy_loaded_esm_aliased_via_import";
  const aliasBody = functionSource(source, aliasTest);
  assert(aliasBody.includes('"custom:aliased" = "lazy_loaded_aliased.js"'));
  assert.equal([...aliasBody.matchAll(/ascii_str_include!\("testdata\/lazy_loaded_importer.js"\)/g)].length, 2);
  assert.equal(typeof readFixture, "function", "included original fixtures require a pinned reader");
  graphs.push({ name: aliasTest, entry: "file:///importer.js", source: readFixture("lazy_loaded_importer.js"),
    dependencies: [{ specifier: "custom:aliased", source: readFixture("lazy_loaded_aliased.js") }] });
  const siblingTest = "test_lazy_load_esm_evaluates_pre_instantiated_sibling";
  const siblingBody = functionSource(source, siblingTest);
  assert(siblingBody.includes('"custom:lazy_a" = "lazy_load_sibling_a.js"'));
  assert(siblingBody.includes('"custom:lazy_b" = "lazy_load_sibling_b.js"'));
  assert.equal([...siblingBody.matchAll(/ascii_str_include!\("testdata\/lazy_load_sibling_main.js"\)/g)].length, 1);
  const siblingB = { specifier: "custom:lazy_b", source: readFixture("lazy_load_sibling_b.js") };
  graphs.push({ name: siblingTest, entry: "file:///main.js", source: readFixture("lazy_load_sibling_main.js"),
    dependencies: [{ specifier: "custom:lazy_a", source: readFixture("lazy_load_sibling_a.js") }, siblingB] });
  graphs.push({ name: siblingTest, entry: siblingB.specifier, source: siblingB.source });
  assert.equal(graphs.length, 11);
  assert.equal(scripts.length, 5);
  return { graphs, scripts };
}
