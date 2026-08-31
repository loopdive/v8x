#!/usr/bin/env node
// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

/**
 * Build the raw inputs for the bounded Deno/js2wasm POC.
 *
 * This deliberately does not precompile Wasm. Native Wasmtime artifacts are
 * target-specific executable code and are produced in the separate trusted
 * packaging phase. The output here is the exact, graph-bound raw Wasm plus a
 * provenance record that the packaging/replay steps validate.
 */

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  readFileSync,
  realpathSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const TOOL_DIR = dirname(fileURLToPath(import.meta.url));
const SCRIPT_V8X_ROOT = realpathSync(resolve(TOOL_DIR, "../.."));

const EXPECTED_JS2_REF = "7bdafea67cf263f923d8039058d99aa6a5720e02";
const EXPECTED_DENO_REF = "1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44";
const WASMTIME_VERSION = "47.0.3";
const TARGET_EXPECTATION = Object.freeze({
  os: "linux",
  arch: "x86_64",
  triple: "x86_64-unknown-linux-gnu",
});
const ENGINE_CONFIG = Object.freeze({
  wasm_function_references: true,
  wasm_gc: true,
  wasm_tail_call: true,
  wasm_exceptions: true,
});
const CANONICALIZATION =
  "UTF-8 recursively lexicographic object keys; array order preserved; no whitespace";

const DENO_INPUTS = Object.freeze([
  {
    gitPath: "libs/core/00_primordials.js",
    path: "00_primordials.js",
    bytes: 19076,
    sha256: "5a2dfbdc4bb81412575d035901a11788001c7e0110e3f736d16289891af44a52",
  },
  {
    gitPath: "libs/core/00_infra.js",
    path: "00_infra.js",
    bytes: 17520,
    sha256: "33984000be930f3b02a2d1149ac0319724e8d95891623c8cc74699da4ce97287",
  },
  {
    gitPath: "libs/core/02_timers.js",
    path: "02_timers.js",
    bytes: 10932,
    sha256: "305596528c679be30d0ac61fa049ec0f1777c287054d119ff4b341575afac7f9",
  },
  {
    gitPath: "libs/core/01_core.js",
    path: "01_core.js",
    bytes: 39939,
    sha256: "6e67972322cc5385a2b642a4f7e941fccb6f992c9de662a5111d11fd0aaf1a3a",
  },
  {
    gitPath: "libs/core/mod.js",
    path: "mod.js",
    bytes: 342,
    sha256: "6850db621a5325d8737ad87d2d24cbc35b7010d5e5f36c88dc53c16610cc40e5",
  },
  {
    gitPath: "libs/core/examples/hello_world.rs",
    path: "hello_world_usage.js",
    bytes: 339,
    sha256: "33bf6b9698833319ad98c0cf88f2fb4dd7634859816ec784aa8902b3eeba1804",
    extract: "hello-world-usage",
  },
]);

const CORE_SCRIPT_INPUTS = DENO_INPUTS.filter((input) => !input.extract);
const LOCK_SOURCE_PATHS = DENO_INPUTS.map((input) => input.path);

// This object is intentionally small and frozen. Its recursively sorted JSON
// is the POC's compile-options commitment; Rust replay requires the digest.
const COMPILE_OPTIONS = Object.freeze({
  target: "standalone",
  platform: "deno",
  externImportModule: "v8x:deno",
  allowJs: true,
  skipSemanticDiagnostics: true,
  deferTopLevelInit: true,
});
const COMPILE_OPTIONS_PREIMAGE = Object.freeze({
  entry: "/v8x-deno-poc/entry.ts",
  compiler: Object.freeze({
    api: "compileMulti",
    provider: "buildRuntimeEvalProviderSource",
    provider_kind: "interpreter",
  }),
  options: COMPILE_OPTIONS,
  source_paths: LOCK_SOURCE_PATHS,
});
const COMPILE_OPTIONS_SHA256 =
  "a31c09c7e31b4852799975e9c8cb8d132aad6ecab79bbf8c98d5848f7c3bde9e";

// This is an ABI bridge, not an implementation of the Deno example. The
// pinned usage source is embedded below and executed through the interpreter
// provider by __v8x_run_classic_script. In particular, do not add a copied
// print/sum sequence here: changing the upstream source must change the graph.
const RUNTIME_SEED = String.raw`
declare function __v8x_deno_sum_begin(isArray: boolean, length: number): void;
declare function __v8x_deno_sum_value(index: number, value: number): void;
declare function __v8x_deno_sum_end(): number;
declare function __v8x_deno_error_kind(): number;
declare function __v8x_deno_error_utf16_length(): number;
declare function __v8x_deno_error_utf16_code_unit(index: number): number;
declare function __v8x_deno_print_begin(isError: boolean, length: number): void;
declare function __v8x_deno_print_code_unit(index: number, value: number): void;
declare function __v8x_deno_print_end(): void;
declare function __v8x_deno_script_utf16_length(): number;
declare function __v8x_deno_script_utf16_code_unit(index: number): number;
declare function __v8x_deno_test_fn_call(): number;
declare function __v8x_deno_test_fn_result_utf16_length(): number;
declare function __v8x_deno_test_fn_result_utf16_code_unit(index: number): number;

function bridgeError(): Error | undefined {
  const kind = __v8x_deno_error_kind();
  if (kind === 0) return undefined;
  let message = "";
  const length = __v8x_deno_error_utf16_length();
  for (let index = 0; index < length; index++) {
    message += String.fromCharCode(__v8x_deno_error_utf16_code_unit(index));
  }
  return kind === 1 ? new TypeError(message) : new Error(message);
}

function finishSum(): number {
  const result = __v8x_deno_sum_end();
  const error = bridgeError();
  if (error !== undefined) throw error;
  return result;
}

export function opSum(values: any): number {
  if (Array.isArray(values)) {
    __v8x_deno_sum_begin(true, values.length);
    for (let index = 0; index < values.length; index++) {
      __v8x_deno_sum_value(index, values[index]);
    }
  } else {
    __v8x_deno_sum_begin(false, 1);
    __v8x_deno_sum_value(0, values);
  }
  return finishSum();
}

export function opPrint(value: any, isError = false): void {
  const message = String(value);
  __v8x_deno_print_begin(isError, message.length);
  for (let index = 0; index < message.length; index++) {
    __v8x_deno_print_code_unit(index, message.charCodeAt(index));
  }
  __v8x_deno_print_end();
  const error = bridgeError();
  if (error !== undefined) throw error;
}

export function readHostScript(): string {
  let source = "";
  const length = __v8x_deno_script_utf16_length();
  for (let index = 0; index < length; index++) {
    source += String.fromCharCode(__v8x_deno_script_utf16_code_unit(index));
  }
  return source;
}

function hostTestFn(): any {
  const status = __v8x_deno_test_fn_call();
  const error = bridgeError();
  if (error !== undefined) throw error;
  if (status === 0) return undefined;
  let encoded = "";
  const length = __v8x_deno_test_fn_result_utf16_length();
  for (let index = 0; index < length; index++) {
    encoded += String.fromCharCode(__v8x_deno_test_fn_result_utf16_code_unit(index));
  }
  return JSON.parse(encoded);
}

function hostTypedArray(kind: string): any {
  return function(values: any[]): any[] {
    (globalThis as any).__v8xLastTypedArrayKind = kind;
    return values;
  };
}

const extrasBinding = {
  getContinuationPreservedEmbedderData() { return undefined; },
  setContinuationPreservedEmbedderData(_value: any) {},
};
const importMetaPrototype: any = {};
const noop = () => {};
const ops: any = {
  op_get_extras_binding_object() { return extrasBinding; },
  op_get_ext_import_meta_proto() { return importMetaPrototype; },
  op_set_captured_bootstrap(bootstrap: any) { (globalThis as any).__capturedBootstrap = bootstrap; },
  op_print: opPrint,
  op_sum: opSum,
};
const core: any = {
  ops,
  print: opPrint,
  callConsole(_v8Method: any, denoMethod: any, ...args: any[]) { return denoMethod(...args); },
};
(globalThis as any).Deno = { core };
(globalThis as any).test_fn = hostTestFn;
(globalThis as any).Uint8Array = hostTypedArray("Uint8Array");
(globalThis as any).Uint16Array = hostTypedArray("Uint16Array");
(globalThis as any).Uint32Array = hostTypedArray("Uint32Array");
(globalThis as any).Int32Array = hostTypedArray("Int32Array");
(globalThis as any).BigUint64Array = hostTypedArray("BigUint64Array");
(globalThis as any).BigInt64Array = hostTypedArray("BigInt64Array");
(globalThis as any).__timers = {
  createTimer: noop,
  cancelTimer: noop,
  refreshTimer: noop,
  refTimer: noop,
  unrefTimer: noop,
  setRunNextTicks: noop,
  setReportException: noop,
  processTimers: noop,
};
`;

function fail(message) {
  throw new Error(`v8x Deno POC builder: ${message}`);
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

function parseArgs(argv) {
  const args = new Map();
  for (const arg of argv) {
    const match = /^--([^=]+)=(.*)$/.exec(arg);
    if (!match) fail(`expected --name=value, received ${arg}`);
    if (args.has(match[1])) fail(`duplicate --${match[1]}`);
    args.set(match[1], match[2]);
  }
  const allowed = new Set([
    "v8x",
    "js2",
    "deno",
    "out",
    "provider-out",
    "provenance-out",
  ]);
  for (const key of args.keys()) {
    if (!allowed.has(key)) fail(`unsupported --${key}`);
  }
  const requiredInput = (name) => {
    const value = args.get(name);
    if (!value) fail(`missing --${name}=PATH`);
    if (!isAbsolute(value)) fail(`--${name} must be absolute`);
    return realpathSync(value);
  };
  const requiredOutput = (name) => {
    const value = args.get(name);
    if (!value) fail(`missing --${name}=PATH`);
    if (!isAbsolute(value)) fail(`--${name} must be absolute`);
    return resolve(value);
  };
  const parsed = {
    v8x: requiredInput("v8x"),
    js2: requiredInput("js2"),
    deno: requiredInput("deno"),
    output: requiredOutput("out"),
    providerOutput: requiredOutput("provider-out"),
    provenanceOutput: requiredOutput("provenance-out"),
  };
  if (
    new Set([parsed.output, parsed.providerOutput, parsed.provenanceOutput])
      .size !== 3
  ) {
    fail(
      "--out, --provider-out, and --provenance-out must name different files",
    );
  }
  return parsed;
}

function git(repo, args, encoding = "utf8") {
  return execFileSync("git", ["-C", repo, ...args], {
    encoding,
    maxBuffer: 32 * 1024 * 1024,
  });
}

function assertCleanDetachedCheckout(label, repo, expectedRef) {
  const inside = git(repo, ["rev-parse", "--is-inside-work-tree"]).trim();
  if (inside !== "true") fail(`${label} is not a Git worktree: ${repo}`);
  const actual = git(repo, ["rev-parse", "HEAD"]).trim();
  if (actual !== expectedRef)
    fail(`${label} checkout is ${actual}, expected ${expectedRef}`);
  try {
    const branch = git(repo, [
      "symbolic-ref",
      "--quiet",
      "--short",
      "HEAD",
    ]).trim();
    fail(`${label} must be detached at ${expectedRef}, found branch ${branch}`);
  } catch (error) {
    if (error?.message?.startsWith("v8x Deno POC builder:")) throw error;
    if (error?.status !== 1) throw error;
  }
  const status = git(repo, [
    "status",
    "--porcelain=v1",
    "--untracked-files=all",
  ]);
  if (status !== "") fail(`${label} checkout is not clean:\n${status}`);
}

function utf8(name, bytes) {
  const text = Buffer.from(bytes).toString("utf8");
  if (!Buffer.from(text, "utf8").equals(bytes))
    fail(`${name} is not canonical UTF-8`);
  return text;
}

function sourceFromPinnedDeno(deno, gitPath) {
  return Buffer.from(
    git(deno, ["show", `${EXPECTED_DENO_REF}:${gitPath}`], null),
  );
}

function extractHelloWorldUsage(exampleBytes) {
  const example = utf8("libs/core/examples/hello_world.rs", exampleBytes);
  const open = /\.execute_script\(\s*"<usage>"\s*,\s*r(#+)?"/g;
  const matches = [...example.matchAll(open)];
  if (matches.length !== 1) {
    fail(
      `expected exactly one <usage> raw Rust string, found ${matches.length}`,
    );
  }
  const match = matches[0];
  const hashes = match[1] ?? "";
  const start = match.index + match[0].length;
  const close = `"${hashes}`;
  const end = example.indexOf(close, start);
  if (end < 0) fail("unterminated <usage> raw Rust string");
  const source = example.slice(start, end);
  if (!example.slice(end + close.length).match(/^\s*,\s*\)/)) {
    fail("<usage> raw Rust string is not the execute_script argument");
  }
  return Buffer.from(source, "utf8");
}

function recordInput(path, bytes, extra = {}) {
  return { path, bytes: bytes.length, sha256: sha256(bytes), ...extra };
}

function recordFile(root, relativePath, role) {
  const bytes = readFileSync(join(root, relativePath));
  return recordInput(relativePath, bytes, { role });
}

function atomicWrite(path, bytes) {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`;
  writeFileSync(temporary, bytes);
  renameSync(temporary, path);
}

function checkedCompile(result, label) {
  const fatal = (result.errors ?? []).filter(
    (error) => error.severity !== "warning",
  );
  const warnings = (result.errors ?? []).filter(
    (error) => error.severity === "warning",
  );
  if (
    !result.success ||
    !result.binary ||
    result.binary.length === 0 ||
    fatal.length > 0
  ) {
    fail(
      `${label} compile failed: ${fatal.map((error) => error.message).join("\n") || "no binary"}`,
    );
  }
  for (const warning of warnings) console.warn(`${label}: ${warning.message}`);
  return Buffer.from(result.binary);
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

async function main() {
  const { v8x, js2, deno, output, providerOutput, provenanceOutput } =
    parseArgs(process.argv.slice(2));
  if (v8x !== SCRIPT_V8X_ROOT) {
    fail(
      `--v8x must identify this builder's checkout (${SCRIPT_V8X_ROOT}), received ${v8x}`,
    );
  }

  // Verify all revisions before reading any tracked source. The v8x ref is
  // intentionally recorded rather than hard-coded: the CI checkout's exact
  // commit is compiled into the replay build and verified from the manifest.
  const v8xRef = git(v8x, ["rev-parse", "HEAD"]).trim();
  assertCleanDetachedCheckout("v8x", v8x, v8xRef);
  assertCleanDetachedCheckout("js2", js2, EXPECTED_JS2_REF);
  assertCleanDetachedCheckout("Deno", deno, EXPECTED_DENO_REF);
  const denoRefFile = readFileSync(
    join(v8x, "tools/deno/DENO_REF"),
    "utf8",
  ).trim();
  if (denoRefFile !== EXPECTED_DENO_REF) {
    fail(
      `tools/deno/DENO_REF is ${denoRefFile}, expected ${EXPECTED_DENO_REF}`,
    );
  }
  for (const name of [
    "JS2WASM_EVAL_ENGINE",
    "TEST262_DISABLE_RUNTIME_EVAL_PROVIDER",
    "TEST262_FULL_RUNTIME_EVAL",
  ]) {
    if (process.env[name] !== undefined)
      fail(
        `${name} is forbidden; this POC always builds the direct interpreter provider`,
      );
  }

  const denoSources = new Map();
  const lockSources = [];
  for (const input of DENO_INPUTS) {
    const raw = sourceFromPinnedDeno(deno, input.gitPath);
    const bytes = input.extract ? extractHelloWorldUsage(raw) : raw;
    if (bytes.length !== input.bytes) {
      fail(`${input.path} has ${bytes.length} bytes, expected ${input.bytes}`);
    }
    const actual = sha256(bytes);
    if (actual !== input.sha256) {
      fail(`${input.path} SHA-256 is ${actual}, expected ${input.sha256}`);
    }
    const source = utf8(input.path, bytes);
    denoSources.set(input.path, source);
    lockSources.push(
      recordInput(input.path, bytes, { git_path: input.gitPath }),
    );
  }

  const exactUsage = denoSources.get("hello_world_usage.js");
  const files = {
    "/v8x-deno-poc/runtime-seed.ts": RUNTIME_SEED,
  };
  for (const input of CORE_SCRIPT_INPUTS) {
    files[`/v8x-deno-poc/core/${input.path}`] = denoSources.get(input.path);
  }
  files["/v8x-deno-poc/entry.ts"] = String.raw`
import { readHostScript } from "./runtime-seed.ts";
import "./core/00_primordials.js";
import "./core/00_infra.js";
import "./core/02_timers.js";
import "./core/01_core.js";
import * as coreModule from "./core/mod.js";

// Value-preserving embedding of the raw Rust literal extracted above. The
// classic-script bridge accepts no alternative source: it first compares the
// host script with these exact bytes, then the interpreter executes this value.
const PINNED_HELLO_WORLD_USAGE = ${JSON.stringify(exactUsage)};
let scriptResult = "";

export function __v8x_probe_deno_core_bootstrap(): number {
  const bootstrap: any = (globalThis as any).__bootstrap;
  const captured: any = (globalThis as any).__capturedBootstrap;
  if (bootstrap == null || captured == null) return 0;
  if (captured.core == null || captured.core.ops == null) return 0;
  if (captured.core === bootstrap.core || captured.core.ops === bootstrap.core.ops) return 0;
  if (captured.core.print !== bootstrap.core.print) return 0;
  if (captured.core.ops.op_print !== bootstrap.core.ops.op_print) return 0;
  if (bootstrap.internals !== captured.internals || bootstrap.primordials !== captured.primordials) return 0;
  if (coreModule.core !== bootstrap.core || coreModule.internals !== bootstrap.internals) return 0;
  if (coreModule.primordials !== bootstrap.primordials) return 0;
  const requiredCoreFunctions = [
    "setUpAsyncStub",
    "__eventLoopTick",
    "__processTimers",
    "__drainNextTickAndMacrotasks",
    "__handleRejections",
    "buildCustomError",
    "runImmediateCallbacks",
    "__setTickInfo",
    "__setImmediateInfo",
    "__setTimerInfo",
  ];
  for (const name of requiredCoreFunctions) {
    if (typeof captured.core[name] !== "function") return 0;
  }
  if (captured.core.errorConstructors == null || typeof captured.core.errorConstructors !== "object") return 0;
  return 42;
}
export function __v8x_run_classic_script(): number {
  const source = readHostScript();
  if (source !== PINNED_HELLO_WORLD_USAGE) {
    throw new Error("v8x Deno POC rejects a classic script outside the pinned hello_world literal");
  }
  (0, eval)(source);
  scriptResult = "";
  return 0;
}
export function __v8x_script_result_utf16_length(): number { return scriptResult.length; }
export function __v8x_script_result_utf16_code_unit(index: number): number {
  return scriptResult.charCodeAt(index);
}
`;

  const graphInputs = [
    ...lockSources,
    recordInput("generated/runtime-seed.ts", Buffer.from(RUNTIME_SEED), {
      role: "abi-bridge",
    }),
    recordInput(
      "generated/entry.ts",
      Buffer.from(files["/v8x-deno-poc/entry.ts"]),
      { role: "closed-world-router" },
    ),
  ];
  const sourceGraphSha256 = inputSetDigest(graphInputs);
  const canonicalCompileOptions = canonicalJson(COMPILE_OPTIONS_PREIMAGE);
  const computedCompileOptionsDigest = sha256(canonicalCompileOptions);
  if (computedCompileOptionsDigest !== COMPILE_OPTIONS_SHA256) {
    fail(
      `compile-options SHA-256 is ${computedCompileOptionsDigest}, expected ${COMPILE_OPTIONS_SHA256}`,
    );
  }

  const compiler = await import(pathToFileURL(join(js2, "src/index.ts")).href);
  const app = await compiler.compileMulti(
    files,
    "/v8x-deno-poc/entry.ts",
    COMPILE_OPTIONS,
  );
  const appBinary = checkedCompile(app, "Deno POC application");
  const appModule = new WebAssembly.Module(appBinary);
  if (
    !WebAssembly.Module.imports(appModule).some(
      (entry) => entry.module === "js2wasm:runtime-eval",
    )
  ) {
    fail(
      "application does not import the interpreter provider; the pinned literal was not routed through it",
    );
  }
  atomicWrite(output, appBinary);

  const provider = await import(
    pathToFileURL(join(js2, "scripts/runtime-eval-provider.mjs")).href
  );
  if (typeof provider.buildRuntimeEvalProviderSource !== "function") {
    fail(
      "js2 runtime-eval provider does not expose buildRuntimeEvalProviderSource()",
    );
  }
  // Never use selectCachedRuntimeEvalProvider(): that selector can choose
  // QuickJS, refusal, or cache fallback. This direct call is interpreter-only.
  const providerSource = provider.buildRuntimeEvalProviderSource();
  const providerResult = await compiler.compile(providerSource, {
    ...provider.RUNTIME_EVAL_PROVIDER_COMPILE_OPTIONS,
  });
  const providerBinary = checkedCompile(
    providerResult,
    "runtime-eval interpreter provider",
  );
  const providerModule = new WebAssembly.Module(providerBinary);
  const providerImports = WebAssembly.Module.imports(providerModule);
  if (providerImports.length !== 0) {
    fail(
      `runtime-eval interpreter provider has ${providerImports.length} imports, expected zero`,
    );
  }
  atomicWrite(providerOutput, providerBinary);

  const acornPinPath = "tests/dogfood/acorn-pin.json";
  const acornPin = JSON.parse(readFileSync(join(js2, acornPinPath), "utf8"));
  if (
    typeof acornPin.tarball !== "string" ||
    !acornPin.tarball.startsWith("fixtures/")
  ) {
    fail(`invalid pinned Acorn tarball path in ${acornPinPath}`);
  }
  const providerInputs = [
    recordFile(js2, "scripts/runtime-eval-provider.mjs", "provider-generator"),
    recordFile(js2, "tests/dogfood/setup-acorn.mjs", "acorn-loader"),
    recordFile(js2, acornPinPath, "acorn-pin"),
    recordFile(
      js2,
      `tests/dogfood/${acornPin.tarball}`,
      "pinned-acorn-tarball",
    ),
    ...[
      "types.ts",
      "opcodes.ts",
      "encoder.ts",
      "runtime-ops.ts",
      "eval-environment.ts",
      "emitter.ts",
      "loop.ts",
      "dynamic-function.ts",
    ].map((name) =>
      recordFile(js2, `src/interp/${name}`, "interpreter-source"),
    ),
  ];
  const providerSourceRecord = recordInput(
    "generated/runtime-eval-provider.ts",
    Buffer.from(providerSource),
    { role: "interpreter-provider-source" },
  );
  const providerGraphSha256 = inputSetDigest([
    providerSourceRecord,
    ...providerInputs,
  ]);
  const rawArtifacts = {
    app: recordInput("deno-core.wasm", appBinary, { role: "app" }),
    runtime_eval_provider: recordInput(
      "runtime-eval-provider.wasm",
      providerBinary,
      { role: "runtime_eval_provider" },
    ),
  };
  const contractPreimage = {
    schema_version: 1,
    revisions: {
      v8x: v8xRef,
      js2: EXPECTED_JS2_REF,
      deno: EXPECTED_DENO_REF,
    },
    sources: lockSources.map(lockedInputRecord),
    source_graph_sha256: sourceGraphSha256,
    provider_graph_sha256: providerGraphSha256,
    compile_options_sha256: computedCompileOptionsDigest,
    wasmtime: {
      version: WASMTIME_VERSION,
      target: TARGET_EXPECTATION,
      engine_config: ENGINE_CONFIG,
    },
    artifacts: {
      app: lockedInputRecord(rawArtifacts.app),
      runtime_eval_provider: lockedInputRecord(
        rawArtifacts.runtime_eval_provider,
      ),
    },
  };
  const contractSha256 = sha256(canonicalJson(contractPreimage));
  const provenance = {
    schema_version: 1,
    kind: "v8x-js2wasm-deno-poc-raw-inputs",
    revisions: {
      v8x: { ref: v8xRef, clean: true, detached: true },
      js2: { ref: EXPECTED_JS2_REF, clean: true, detached: true },
      deno: { ref: EXPECTED_DENO_REF, clean: true, detached: true },
    },
    sources: lockSources,
    source_graph: {
      entry: "/v8x-deno-poc/entry.ts",
      sha256: sourceGraphSha256,
      inputs: graphInputs,
    },
    compiler: {
      js2_index: recordFile(js2, "src/index.ts", "compiler-entry"),
      package_json: recordFile(js2, "package.json", "compiler-package"),
      pnpm_lock: recordFile(js2, "pnpm-lock.yaml", "compiler-lock"),
      runtime_eval_provider: {
        kind: "interpreter",
        direct_builder: "buildRuntimeEvalProviderSource",
        source: providerSourceRecord,
        inputs: providerInputs,
        sha256: providerGraphSha256,
      },
    },
    compile_options: {
      canonicalization: CANONICALIZATION,
      canonical_json: canonicalCompileOptions,
      sha256: computedCompileOptionsDigest,
    },
    wasmtime: {
      version: WASMTIME_VERSION,
      target_expectation: TARGET_EXPECTATION,
      engine_config: ENGINE_CONFIG,
    },
    artifacts: rawArtifacts,
    contract: {
      canonicalization: CANONICALIZATION,
      sha256: contractSha256,
    },
  };
  atomicWrite(provenanceOutput, `${JSON.stringify(provenance, null, 2)}\n`);

  console.log(
    JSON.stringify(
      {
        provenance: provenanceOutput,
        app: {
          path: output,
          bytes: appBinary.length,
          sha256: sha256(appBinary),
        },
        runtime_eval_provider: {
          path: providerOutput,
          bytes: providerBinary.length,
          sha256: sha256(providerBinary),
        },
      },
      null,
      2,
    ),
  );
}

main().catch((error) => {
  console.error(error?.stack ?? error);
  process.exitCode = 1;
});
