#!/usr/bin/env node
// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

/**
 * Convert trusted raw-build provenance plus same-host AOT files into the
 * intentionally narrow replay manifest consumed by the compiler-free v8x
 * Deno POC. The replay manifest has no raw-Wasm path: raw bytes are a build
 * provenance input only and are removed before the fresh replay process.
 */

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";

const EXPECTED = Object.freeze({
  canonicalization:
    "UTF-8 recursively lexicographic object keys; array order preserved; no whitespace",
  js2Ref: "00d0cc0352bd456e81fdfcf66f5a2e5f86cb0deb",
  denoRef: "1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44",
  wasmtime: "47.0.3",
  compileOptions:
    "a31c09c7e31b4852799975e9c8cb8d132aad6ecab79bbf8c98d5848f7c3bde9e",
  sources: Object.freeze([
    {
      path: "00_primordials.js",
      git_path: "libs/core/00_primordials.js",
      bytes: 19076,
      sha256:
        "5a2dfbdc4bb81412575d035901a11788001c7e0110e3f736d16289891af44a52",
    },
    {
      path: "00_infra.js",
      git_path: "libs/core/00_infra.js",
      bytes: 17520,
      sha256:
        "33984000be930f3b02a2d1149ac0319724e8d95891623c8cc74699da4ce97287",
    },
    {
      path: "02_timers.js",
      git_path: "libs/core/02_timers.js",
      bytes: 10932,
      sha256:
        "305596528c679be30d0ac61fa049ec0f1777c287054d119ff4b341575afac7f9",
    },
    {
      path: "01_core.js",
      git_path: "libs/core/01_core.js",
      bytes: 39939,
      sha256:
        "6e67972322cc5385a2b642a4f7e941fccb6f992c9de662a5111d11fd0aaf1a3a",
    },
    {
      path: "mod.js",
      git_path: "libs/core/mod.js",
      bytes: 342,
      sha256:
        "6850db621a5325d8737ad87d2d24cbc35b7010d5e5f36c88dc53c16610cc40e5",
    },
    {
      path: "hello_world_usage.js",
      git_path: "libs/core/examples/hello_world.rs",
      bytes: 339,
      sha256:
        "33bf6b9698833319ad98c0cf88f2fb4dd7634859816ec784aa8902b3eeba1804",
    },
  ]),
  target: Object.freeze({
    os: "linux",
    arch: "x86_64",
    triple: "x86_64-unknown-linux-gnu",
  }),
  engineConfig: Object.freeze({
    wasm_function_references: true,
    wasm_gc: true,
    wasm_tail_call: true,
    wasm_exceptions: true,
    debug_symbols: false,
    generate_address_map: false,
    wasm_backtrace_details: "disable",
  }),
});

function fail(message) {
  throw new Error(`v8x Deno POC lock finalizer: ${message}`);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function canonicalJson(value) {
  if (
    value === null ||
    typeof value === "boolean" ||
    typeof value === "string" ||
    typeof value === "number"
  ) {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  fail(`cannot canonicalize ${typeof value}`);
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function requireRecord(value, label) {
  if (!isRecord(value)) fail(`${label} must be an object`);
  return value;
}

function requireExactKeys(record, keys, label) {
  const expected = new Set(keys);
  const actual = Object.keys(record);
  if (
    actual.length !== expected.size ||
    actual.some((key) => !expected.has(key))
  ) {
    fail(`${label} has an unexpected field inventory`);
  }
  return record;
}

function requireSha(value, label) {
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/.test(value)) {
    fail(`${label} must be a lowercase SHA-256 hex digest`);
  }
  return value;
}

function lockedInputRecord(record) {
  const locked = {
    path: record.path,
    bytes: record.bytes,
    sha256: record.sha256,
  };
  if (record.git_path !== undefined) locked.git_path = record.git_path;
  if (record.role !== undefined) locked.role = record.role;
  return locked;
}

function inputSetDigest(inputs) {
  return sha256(canonicalJson(inputs.map(lockedInputRecord)));
}

function validateInputRecord(record, label, expected = {}) {
  requireRecord(record, label);
  const permitted = ["path", "bytes", "sha256"];
  if (expected.gitPath !== undefined) permitted.push("git_path");
  if (expected.role !== undefined) permitted.push("role");
  requireExactKeys(record, permitted, label);
  if (typeof record.path !== "string" || record.path.length === 0) {
    fail(`${label}.path must be a non-empty string`);
  }
  if (!Number.isSafeInteger(record.bytes) || record.bytes <= 0) {
    fail(`${label}.bytes must be a positive safe integer`);
  }
  requireSha(record.sha256, `${label}.sha256`);
  if (expected.path !== undefined && record.path !== expected.path) {
    fail(
      `${label}.path is ${JSON.stringify(record.path)}, expected ${expected.path}`,
    );
  }
  if (expected.gitPath !== undefined && record.git_path !== expected.gitPath) {
    fail(
      `${label}.git_path is ${JSON.stringify(record.git_path)}, expected ${expected.gitPath}`,
    );
  }
  if (expected.role !== undefined && record.role !== expected.role) {
    fail(
      `${label}.role is ${JSON.stringify(record.role)}, expected ${expected.role}`,
    );
  }
  return lockedInputRecord(record);
}

function requireAbsolute(name, value, output = false) {
  if (!value) fail(`missing --${name}=PATH`);
  if (!isAbsolute(value)) fail(`--${name} must be absolute`);
  return output ? resolve(value) : resolve(value);
}

function parseArgs(argv) {
  const args = new Map();
  for (const arg of argv) {
    const match = /^--([^=]+)=(.*)$/.exec(arg);
    if (!match) fail(`expected --name=value, received ${arg}`);
    if (args.has(match[1])) fail(`duplicate --${match[1]}`);
    args.set(match[1], match[2]);
  }
  const allowed = new Set([
    "provenance",
    "app-raw",
    "provider-raw",
    "app-aot",
    "provider-aot",
    "app-attestation",
    "provider-attestation",
    "out",
  ]);
  for (const name of args.keys()) {
    if (!allowed.has(name)) fail(`unsupported --${name}`);
  }
  const parsed = {
    provenance: requireAbsolute("provenance", args.get("provenance")),
    appRaw: requireAbsolute("app-raw", args.get("app-raw")),
    providerRaw: requireAbsolute("provider-raw", args.get("provider-raw")),
    appAot: requireAbsolute("app-aot", args.get("app-aot")),
    providerAot: requireAbsolute("provider-aot", args.get("provider-aot")),
    appAttestation: requireAbsolute(
      "app-attestation",
      args.get("app-attestation"),
    ),
    providerAttestation: requireAbsolute(
      "provider-attestation",
      args.get("provider-attestation"),
    ),
    out: requireAbsolute("out", args.get("out"), true),
  };
  for (const [role, aot, attestation] of [
    ["app", parsed.appAot, parsed.appAttestation],
    ["runtime_eval_provider", parsed.providerAot, parsed.providerAttestation],
  ]) {
    const expected = `${aot}.attestation.json`;
    if (attestation !== expected) {
      fail(
        `${role} attestation path is ${JSON.stringify(attestation)}, expected ${expected}`,
      );
    }
  }
  return parsed;
}

function readJson(path, label) {
  let parsed;
  try {
    parsed = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    fail(`read ${label} ${path}: ${error.message}`);
  }
  return requireRecord(parsed, label);
}

function rawArtifact(provenance, role, expectedPath, rawPath) {
  const artifacts = requireRecord(
    provenance.artifacts,
    "raw provenance artifacts",
  );
  requireExactKeys(
    artifacts,
    ["app", "runtime_eval_provider"],
    "raw provenance artifacts",
  );
  const declared = validateInputRecord(
    artifacts[role],
    `raw provenance artifacts.${role}`,
    { path: expectedPath, role },
  );
  const bytes = readFileSync(rawPath);
  if (bytes.length === 0) fail(`${role} raw Wasm is empty: ${rawPath}`);
  const actual = sha256(bytes);
  if (bytes.length !== declared.bytes) {
    fail(
      `${role} raw Wasm has ${bytes.length} bytes, expected ${declared.bytes}`,
    );
  }
  if (actual !== declared.sha256) {
    fail(`${role} raw Wasm digest is ${actual}, expected ${declared.sha256}`);
  }
  return declared;
}

function validateTarget(target, label) {
  requireExactKeys(
    requireRecord(target, label),
    ["os", "arch", "triple"],
    label,
  );
  for (const [key, expected] of Object.entries(EXPECTED.target)) {
    if (target[key] !== expected) {
      fail(
        `${label}.${key} is ${JSON.stringify(target[key])}, expected ${expected}`,
      );
    }
  }
  return { ...EXPECTED.target };
}

function validateEngineConfig(config, label) {
  requireExactKeys(
    requireRecord(config, label),
    Object.keys(EXPECTED.engineConfig),
    label,
  );
  for (const [name, expected] of Object.entries(EXPECTED.engineConfig)) {
    if (config[name] !== expected) {
      fail(`${label}.${name} must be ${JSON.stringify(expected)}`);
    }
  }
  return { ...EXPECTED.engineConfig };
}

function attestedArtifact({ role, raw, rawBytes, aotBytes, path }) {
  const label = `${role} AOT attestation`;
  const attestation = readJson(path, label);
  requireExactKeys(
    attestation,
    [
      "schema_version",
      "role",
      "wasmtime_version",
      "target",
      "engine_config",
      "raw_sha256",
      "aot_sha256",
    ],
    label,
  );
  if (attestation.schema_version !== 1) {
    fail(`${label}.schema_version must be 1`);
  }
  if (attestation.role !== role) {
    fail(
      `${label}.role is ${JSON.stringify(attestation.role)}, expected ${role}`,
    );
  }
  if (attestation.wasmtime_version !== EXPECTED.wasmtime) {
    fail(
      `${label}.wasmtime_version is ${JSON.stringify(attestation.wasmtime_version)}, expected ${EXPECTED.wasmtime}`,
    );
  }
  validateTarget(attestation.target, `${label}.target`);
  validateEngineConfig(attestation.engine_config, `${label}.engine_config`);
  const rawSha256 = requireSha(attestation.raw_sha256, `${label}.raw_sha256`);
  const aotSha256 = requireSha(attestation.aot_sha256, `${label}.aot_sha256`);
  const actualRawSha256 = sha256(rawBytes);
  const actualAotSha256 = sha256(aotBytes);
  if (actualRawSha256 !== raw.sha256 || rawSha256 !== raw.sha256) {
    fail(
      `${label} raw_sha256 does not bind the declared and supplied ${role} raw Wasm`,
    );
  }
  if (aotSha256 !== actualAotSha256) {
    fail(`${label} aot_sha256 does not bind the supplied ${role} AOT bytes`);
  }
  return {
    raw_sha256: rawSha256,
    aot_sha256: aotSha256,
    attestation_sha256: sha256(canonicalJson(attestation)),
  };
}

function validateProvider(provider) {
  requireExactKeys(
    provider,
    ["kind", "direct_builder", "source", "inputs", "sha256"],
    "raw provenance interpreter provider",
  );
  if (
    provider.kind !== "interpreter" ||
    provider.direct_builder !== "buildRuntimeEvalProviderSource"
  ) {
    fail("raw provenance selected a non-interpreter runtime-eval provider");
  }
  const source = validateInputRecord(
    provider.source,
    "raw provenance interpreter provider.source",
    {
      path: "generated/runtime-eval-provider.ts",
      role: "interpreter-provider-source",
    },
  );
  if (!Array.isArray(provider.inputs) || provider.inputs.length !== 12) {
    fail("raw provenance interpreter provider has the wrong input inventory");
  }
  const expected = [
    ["scripts/runtime-eval-provider.mjs", "provider-generator"],
    ["tests/dogfood/setup-acorn.mjs", "acorn-loader"],
    ["tests/dogfood/acorn-pin.json", "acorn-pin"],
    ["tests/dogfood/fixtures/acorn-8.16.0.tgz", "pinned-acorn-tarball"],
    ...[
      "types.ts",
      "opcodes.ts",
      "encoder.ts",
      "runtime-ops.ts",
      "eval-environment.ts",
      "emitter.ts",
      "loop.ts",
      "dynamic-function.ts",
    ].map((name) => [`src/interp/${name}`, "interpreter-source"]),
  ];
  const inputs = provider.inputs.map((input, index) => {
    const [path, role] = expected[index];
    const record = validateInputRecord(
      input,
      `raw provenance interpreter provider.inputs[${index}]`,
      { ...(path === undefined ? {} : { path }), role },
    );
    return record;
  });
  const actual = inputSetDigest([source, ...inputs]);
  const declared = requireSha(
    provider.sha256,
    "raw provenance interpreter provider.sha256",
  );
  if (actual !== declared) {
    fail(
      `raw provenance interpreter provider digest is ${actual}, expected ${declared}`,
    );
  }
  return actual;
}

function validateProvenance(provenance) {
  requireExactKeys(
    provenance,
    [
      "schema_version",
      "kind",
      "revisions",
      "sources",
      "source_graph",
      "compiler",
      "compile_options",
      "wasmtime",
      "artifacts",
      "contract",
    ],
    "raw provenance",
  );
  if (
    provenance.schema_version !== 1 ||
    provenance.kind !== "v8x-js2wasm-deno-poc-raw-inputs"
  ) {
    fail("raw provenance is not schema v1 v8x-js2wasm-deno-poc-raw-inputs");
  }
  const revisions = requireRecord(
    provenance.revisions,
    "raw provenance revisions",
  );
  requireExactKeys(
    revisions,
    ["v8x", "js2", "deno"],
    "raw provenance revisions",
  );
  const v8x = requireRecord(revisions.v8x, "raw provenance revisions.v8x");
  const js2 = requireRecord(revisions.js2, "raw provenance revisions.js2");
  const deno = requireRecord(revisions.deno, "raw provenance revisions.deno");
  for (const [name, revision] of Object.entries({ v8x, js2, deno })) {
    requireExactKeys(
      revision,
      ["ref", "clean", "detached"],
      `raw provenance revisions.${name}`,
    );
  }
  if (
    !/^[0-9a-f]{40}$/.test(v8x.ref) ||
    v8x.clean !== true ||
    v8x.detached !== true
  ) {
    fail("raw provenance does not bind a clean detached v8x commit");
  }
  if (
    js2.ref !== EXPECTED.js2Ref ||
    js2.clean !== true ||
    js2.detached !== true
  ) {
    fail("raw provenance does not bind the clean pinned js2 checkout");
  }
  if (
    deno.ref !== EXPECTED.denoRef ||
    deno.clean !== true ||
    deno.detached !== true
  ) {
    fail("raw provenance does not bind the clean pinned Deno checkout");
  }
  if (
    !Array.isArray(provenance.sources) ||
    provenance.sources.length !== EXPECTED.sources.length
  ) {
    fail("raw provenance has the wrong source inventory");
  }
  const sources = provenance.sources.map((source, index) => {
    const expected = EXPECTED.sources[index];
    const record = validateInputRecord(
      source,
      `raw provenance sources[${index}]`,
      { path: expected.path, gitPath: expected.git_path },
    );
    if (record.bytes !== expected.bytes || record.sha256 !== expected.sha256) {
      fail(
        `raw provenance source ${index} is not the pinned ${expected.path} input`,
      );
    }
    return record;
  });
  const sourceGraph = requireRecord(
    provenance.source_graph,
    "raw provenance source_graph",
  );
  requireExactKeys(
    sourceGraph,
    ["entry", "sha256", "inputs"],
    "raw provenance source_graph",
  );
  if (sourceGraph.entry !== "/v8x-deno-poc/entry.ts") {
    fail(
      "raw provenance source_graph.entry is not the closed-world entrypoint",
    );
  }
  if (!Array.isArray(sourceGraph.inputs) || sourceGraph.inputs.length !== 8) {
    fail("raw provenance source_graph has the wrong input inventory");
  }
  const graphInputs = sourceGraph.inputs.map((input, index) => {
    if (index < sources.length) {
      const expected = sources[index];
      const record = validateInputRecord(
        input,
        `raw provenance source_graph.inputs[${index}]`,
        { path: expected.path, gitPath: expected.git_path },
      );
      if (
        record.bytes !== expected.bytes ||
        record.sha256 !== expected.sha256
      ) {
        fail(`raw provenance source_graph input ${index} differs from sources`);
      }
      return record;
    }
    return validateInputRecord(
      input,
      `raw provenance source_graph.inputs[${index}]`,
      index === 6
        ? { path: "generated/runtime-seed.ts", role: "abi-bridge" }
        : { path: "generated/entry.ts", role: "closed-world-router" },
    );
  });
  const sourceGraphSha256 = requireSha(
    sourceGraph.sha256,
    "raw provenance source_graph.sha256",
  );
  const computedSourceGraphSha256 = inputSetDigest(graphInputs);
  if (computedSourceGraphSha256 !== sourceGraphSha256) {
    fail(
      `raw provenance source graph digest is ${computedSourceGraphSha256}, expected ${sourceGraphSha256}`,
    );
  }
  const options = requireRecord(
    provenance.compile_options,
    "raw provenance compile_options",
  );
  requireExactKeys(
    options,
    ["canonicalization", "canonical_json", "sha256"],
    "raw provenance compile_options",
  );
  if (options.canonicalization !== EXPECTED.canonicalization) {
    fail("raw provenance compile_options uses an unknown canonicalization");
  }
  if (typeof options.canonical_json !== "string") {
    fail("raw provenance compile_options.canonical_json must be a string");
  }
  let parsedOptions;
  try {
    parsedOptions = JSON.parse(options.canonical_json);
  } catch (error) {
    fail(
      `raw provenance compile_options.canonical_json is invalid: ${error.message}`,
    );
  }
  if (canonicalJson(parsedOptions) !== options.canonical_json) {
    fail("raw provenance compile_options.canonical_json is not canonical");
  }
  if (sha256(Buffer.from(options.canonical_json)) !== options.sha256) {
    fail("raw provenance compile_options.canonical_json digest mismatch");
  }
  if (options.sha256 !== EXPECTED.compileOptions) {
    fail(
      `raw provenance compile-options digest is ${options.sha256}, expected ${EXPECTED.compileOptions}`,
    );
  }
  const wasmtime = requireRecord(
    provenance.wasmtime,
    "raw provenance wasmtime",
  );
  requireExactKeys(
    wasmtime,
    ["version", "target_expectation", "engine_config"],
    "raw provenance wasmtime",
  );
  if (wasmtime.version !== EXPECTED.wasmtime) {
    fail("raw provenance has an incompatible Wasmtime version");
  }
  const target = validateTarget(
    wasmtime.target_expectation,
    "raw provenance Wasmtime target expectation",
  );
  const engineConfig = validateEngineConfig(
    wasmtime.engine_config,
    "raw provenance Wasmtime engine config",
  );
  const compiler = requireRecord(
    provenance.compiler,
    "raw provenance compiler",
  );
  requireExactKeys(
    compiler,
    ["js2_index", "package_json", "pnpm_lock", "runtime_eval_provider"],
    "raw provenance compiler",
  );
  validateInputRecord(compiler.js2_index, "raw provenance compiler.js2_index", {
    path: "src/index.ts",
    role: "compiler-entry",
  });
  validateInputRecord(
    compiler.package_json,
    "raw provenance compiler.package_json",
    { path: "package.json", role: "compiler-package" },
  );
  validateInputRecord(compiler.pnpm_lock, "raw provenance compiler.pnpm_lock", {
    path: "pnpm-lock.yaml",
    role: "compiler-lock",
  });
  const provider = requireRecord(
    compiler.runtime_eval_provider,
    "raw provenance interpreter provider",
  );
  const providerGraphSha256 = validateProvider(provider);
  const contract = requireExactKeys(
    requireRecord(provenance.contract, "raw provenance contract"),
    ["canonicalization", "sha256"],
    "raw provenance contract",
  );
  if (contract.canonicalization !== EXPECTED.canonicalization) {
    fail("raw provenance contract uses an unknown canonicalization");
  }
  requireSha(contract.sha256, "raw provenance contract.sha256");
  return {
    v8xRef: v8x.ref,
    sources,
    sourceGraphSha256,
    providerGraphSha256,
    target,
    engineConfig,
    rawContractSha256: contract.sha256,
  };
}

function atomicWrite(path, text) {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`;
  writeFileSync(temporary, text);
  renameSync(temporary, path);
}

function main() {
  const paths = parseArgs(process.argv.slice(2));
  if (process.platform !== "linux" || process.arch !== "x64") {
    fail(
      `this POC only finalizes Linux x86_64 artifacts, got ${process.platform}/${process.arch}`,
    );
  }
  if (new Set(Object.values(paths)).size !== Object.keys(paths).length) {
    fail("all inputs and --out must be distinct paths");
  }
  const provenance = readJson(paths.provenance, "raw provenance");
  const {
    v8xRef,
    sources,
    sourceGraphSha256,
    providerGraphSha256,
    target,
    engineConfig,
    rawContractSha256,
  } = validateProvenance(provenance);
  const appRaw = rawArtifact(provenance, "app", "deno-core.wasm", paths.appRaw);
  const providerRaw = rawArtifact(
    provenance,
    "runtime_eval_provider",
    "runtime-eval-provider.wasm",
    paths.providerRaw,
  );
  const appRawBytes = readFileSync(paths.appRaw);
  const providerRawBytes = readFileSync(paths.providerRaw);
  const appAot = readFileSync(paths.appAot);
  const providerAot = readFileSync(paths.providerAot);
  if (appAot.length === 0 || providerAot.length === 0)
    fail("AOT artifact is empty");
  const appAttestation = attestedArtifact({
    role: "app",
    raw: appRaw,
    rawBytes: appRawBytes,
    aotBytes: appAot,
    path: paths.appAttestation,
  });
  const providerAttestation = attestedArtifact({
    role: "runtime_eval_provider",
    raw: providerRaw,
    rawBytes: providerRawBytes,
    aotBytes: providerAot,
    path: paths.providerAttestation,
  });
  const contractPreimage = {
    schema_version: 1,
    revisions: {
      v8x: v8xRef,
      js2: EXPECTED.js2Ref,
      deno: EXPECTED.denoRef,
    },
    sources: sources.map(lockedInputRecord),
    source_graph_sha256: sourceGraphSha256,
    provider_graph_sha256: providerGraphSha256,
    compile_options_sha256: EXPECTED.compileOptions,
    wasmtime: {
      version: EXPECTED.wasmtime,
      target,
      engine_config: engineConfig,
    },
    artifacts: {
      app: lockedInputRecord(appRaw),
      runtime_eval_provider: lockedInputRecord(providerRaw),
    },
  };
  const computedContractSha256 = sha256(canonicalJson(contractPreimage));
  if (computedContractSha256 !== rawContractSha256) {
    fail(
      `raw provenance contract digest is ${computedContractSha256}, expected ${rawContractSha256}`,
    );
  }

  const artifacts = {
    app: {
      role: "app",
      raw_path: "deno-core.wasm",
      ...appAttestation,
    },
    runtime_eval_provider: {
      role: "runtime_eval_provider",
      raw_path: "runtime-eval-provider.wasm",
      ...providerAttestation,
    },
  };
  const lockPreimage = {
    schema_version: 1,
    raw_contract_sha256: computedContractSha256,
    deno_ref: EXPECTED.denoRef,
    js2_ref: EXPECTED.js2Ref,
    v8x_ref: v8xRef,
    wasmtime_version: EXPECTED.wasmtime,
    target,
    engine_config: engineConfig,
    sources: sources.map(({ path, bytes, sha256: digest }) => ({
      path,
      bytes,
      sha256: digest,
    })),
    compile_options_sha256: EXPECTED.compileOptions,
    artifacts,
  };
  const replayContractSha256 = sha256(canonicalJson(lockPreimage));

  // Do not add fields to this object without changing the Rust parser: it
  // rejects unknown fields on purpose, so lock consumers cannot silently drift.
  const lock = {
    ...lockPreimage,
    contract_sha256: replayContractSha256,
  };
  atomicWrite(paths.out, `${JSON.stringify(lock, null, 2)}\n`);
  process.stdout.write(
    `${JSON.stringify({ manifest: paths.out, v8x_ref: v8xRef }, null, 2)}\n`,
  );
}

try {
  main();
} catch (error) {
  console.error(error?.stack ?? error);
  process.exitCode = 1;
}
