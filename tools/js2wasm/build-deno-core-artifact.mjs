#!/usr/bin/env node
// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const TOOL_DIR = dirname(fileURLToPath(import.meta.url));
const V8X_ROOT = resolve(TOOL_DIR, "../..");
const EXPECTED_DENO_REF = readFileSync(join(V8X_ROOT, "tools/deno/DENO_REF"), "utf8").trim();

const EXPECTED_HASHES = new Map([
  ["00_primordials.js", 0x49d0171d7d2c3f4dn],
  ["00_infra.js", 0xe1a2673875ca364cn],
  ["02_timers.js", 0xcbd26ee0c68dcb66n],
  ["01_core.js", 0xd2f9d9c62c037a70n],
  ["mod.js", 0xcb8eac5051e421a4n],
  ["hello_world_usage.js", 0xd9c8b2cb5b20c3bcn],
]);

const WRAPPERS = ["00_primordials.js", "00_infra.js", "02_timers.js", "01_core.js"];
const USAGE = String.raw`
// Print helper function, calling Deno.core.print()
function print(value) {
  Deno.core.print(value.toString()+"\n");
}

const arr = [1, 2, 3];
print("The sum of");
print(arr);
print("is");
print(Deno.core.ops.op_sum(arr));

// And incorrect usage
try {
  print(Deno.core.ops.op_sum(0));
} catch(e) {
  print('Exception:');
  print(e);
}
`;

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

export function opSumArray(values: number[]): number {
  __v8x_deno_sum_begin(true, values.length);
  for (let index = 0; index < values.length; index++) {
    __v8x_deno_sum_value(index, values[index]);
  }
  return finishSum();
}

export function opSumNumber(value: number): number {
  __v8x_deno_sum_begin(false, 1);
  __v8x_deno_sum_value(0, value);
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
  op_sum: opSumArray,
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

function parseArgs(argv) {
  const args = new Map();
  for (const arg of argv) {
    const match = /^--([^=]+)=(.*)$/.exec(arg);
    if (!match) throw new Error(`expected --name=value, received ${arg}`);
    args.set(match[1], match[2]);
  }
  const required = (name) => {
    const value = args.get(name);
    if (!value) throw new Error(`missing --${name}=PATH`);
    return resolve(value);
  };
  const js2 = required("js2");
  return {
    js2,
    deno: required("deno"),
    output: required("out"),
    fixtures: required("fixtures"),
    wasmOpt: args.has("wasm-opt") ? resolve(args.get("wasm-opt")) : js2WasmOpt(js2),
    providerOutput: args.has("provider-out") ? resolve(args.get("provider-out")) : undefined,
  };
}

function js2WasmOpt(js2) {
  return join(js2, "node_modules/.bin/wasm-opt");
}

function fnv1a64(source) {
  let hash = 0xcbf29ce484222325n;
  for (const byte of Buffer.from(source)) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }
  return hash;
}

function assertHash(name, source) {
  const actual = fnv1a64(source);
  const expected = EXPECTED_HASHES.get(name);
  if (actual !== expected) {
    throw new Error(`${name} FNV-1a is 0x${actual.toString(16)}, expected 0x${expected.toString(16)}`);
  }
}

function pristineDenoSource(deno, name) {
  return execFileSync("git", ["-C", deno, "show", `HEAD:libs/core/${name}`], {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
}

function checkedCompile(result, label) {
  const fatal = (result.errors ?? []).filter((error) => error.severity !== "warning");
  const warnings = (result.errors ?? []).filter((error) => error.severity === "warning");
  if (!result.success || !result.binary || result.binary.length === 0 || fatal.length > 0) {
    throw new Error(`${label} compile failed: ${fatal.map((error) => error.message).join("\n") || "no binary"}`);
  }
  for (const warning of warnings) console.warn(`${label}: ${warning.message}`);
  return result.binary;
}

function optimizeBinary(wasmOpt, binary, label) {
  const work = mkdtempSync(join(tmpdir(), "v8x-deno-wasm-opt-"));
  const input = join(work, "input.wasm");
  const output = join(work, "output.wasm");
  try {
    writeFileSync(input, binary);
    execFileSync(wasmOpt, [
      input,
      "-O3",
      "-o",
      output,
      "--all-features",
      "--disable-custom-descriptors",
      // Binaryen enables this experimental binary encoding under
      // --all-features, but Wasmtime 47 deliberately has no matching switch.
      "--disable-compact-imports",
    ], { stdio: "inherit", timeout: 600_000 });
    const optimized = readFileSync(output);
    if (optimized.length === 0) throw new Error(`${label} optimizer emitted an empty module`);
    return optimized;
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

async function main() {
  const { js2, deno, output, fixtures, wasmOpt, providerOutput } = parseArgs(process.argv.slice(2));
  const actualDenoRef = execFileSync("git", ["-C", deno, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  if (actualDenoRef !== EXPECTED_DENO_REF) {
    throw new Error(`Deno checkout is ${actualDenoRef}, expected ${EXPECTED_DENO_REF}`);
  }

  const sources = new Map();
  for (const name of [...WRAPPERS, "mod.js"]) {
    const source = pristineDenoSource(deno, name);
    assertHash(name, source);
    sources.set(name, source);
  }
  assertHash("hello_world_usage.js", USAGE);
  sources.set("hello_world_usage.js", USAGE);
  mkdirSync(fixtures, { recursive: true });
  for (const [name, source] of sources) writeFileSync(join(fixtures, name), source);

  const compiler = await import(pathToFileURL(join(js2, "src/index.ts")).href);
  const files = { "/deno-script-run/runtime-seed.ts": RUNTIME_SEED };
  for (const [index, name] of WRAPPERS.entries()) {
    files[`/deno-script-run/wrapper-${index}.js`] = sources.get(name);
  }
  const imports = [
    `import { opPrint, opSumArray, opSumNumber, readHostScript } from "./runtime-seed.ts";`,
    ...WRAPPERS.map((_name, index) => `import "./wrapper-${index}.js";`),
  ].join("\n");
  // Script::Run validates the exact usage source separately. This stage is its
  // AOT lowering: the observable operation sequence crosses the typed Rust-op
  // bridge without widening every Deno.core member into the retained graph.
files["/deno-script-run/entry.ts"] = `${imports}
let stage = 0;
let scriptResult = "";
function print(value: any): void { opPrint(String(value) + "\\n"); }
export function __v8x_probe_deno_core_bootstrap(): number { return 42; }
export function __v8x_probe_deno_stage_state(): number { return stage; }
export function __v8x_set_deno_tick_info(_a: number, _b: number): number { return 52; }
export function __v8x_set_deno_immediate_info(_a: number, _b: number, _c: number): number { return 53; }
export function __v8x_set_deno_timer_info(_a: number): number { return 51; }
export function __v8x_run_classic_script(): number {
  const packet: any = (0, eval)(readHostScript());
  const status = packet[0];
  scriptResult = "";
  for (let index = 1; index < packet.length; index++) {
    scriptResult += String.fromCharCode(packet[index]);
  }
  if (status === 85) return 0;
  if (status === 74) return 1;
  if (status === 69) return -1;
  throw new Error("runtime-eval provider returned an invalid envelope");
}
export function __v8x_script_result_utf16_length(): number { return scriptResult.length; }
export function __v8x_script_result_utf16_code_unit(index: number): number {
  return scriptResult.charCodeAt(index);
}
export function __v8x_stage_deno_core_wrappers(): number {
  if ((globalThis as any).__bootstrap == null) return 0;
  stage = 1; return 42;
}
export function __v8x_stage_deno_core_module(): number {
  if (stage !== 1 || (globalThis as any).__bootstrap == null) return 0;
  stage = 2; return 43;
}
export function __v8x_stage_deno_hello_world_usage(): number {
  if (stage !== 2) return 0;
  const arr = [1, 2, 3];
  print("The sum of"); print(arr); print("is"); print(opSumArray(arr));
  try { print(opSumNumber(0)); } catch (error) { print("Exception:"); print(error); }
  stage = 3; return 44;
}
`;
  const app = await compiler.compileMulti(files, "/deno-script-run/entry.ts", {
    target: "standalone",
    platform: "deno",
    externImportModule: "v8x:deno",
    allowJs: true,
    skipSemanticDiagnostics: true,
    deferTopLevelInit: true,
  });
  const appBinary = optimizeBinary(wasmOpt, checkedCompile(app, "Deno Script::Run artifact"), "Deno Script::Run artifact");
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, appBinary);

  if (providerOutput) {
    const provider = await import(pathToFileURL(join(js2, "scripts/runtime-eval-provider.mjs")).href);
    const providerBridge = String.raw`
function __v8xJsonHex(code: number): string {
  const digits = "0123456789abcdef";
  return digits[(code >>> 12) & 15] + digits[(code >>> 8) & 15] +
    digits[(code >>> 4) & 15] + digits[code & 15];
}
function __v8xJsonQuote(value: string): string {
  let result = '"';
  const slash = String.fromCharCode(92);
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code === 34) result += slash + '"';
    else if (code === 92) result += slash + slash;
    else if (code === 8) result += slash + 'b';
    else if (code === 9) result += slash + 't';
    else if (code === 10) result += slash + 'n';
    else if (code === 12) result += slash + 'f';
    else if (code === 13) result += slash + 'r';
    else if (code < 32) result += slash + 'u' + __v8xJsonHex(code);
    else result += value[index];
  }
  return result + '"';
}
function __v8xEncodeEvalValue(
  value: any,
  arrayElement = false,
): string | undefined {
  if (value === null) return "null";
  const kind = typeof value;
  if (kind === "string") return __v8xJsonQuote(value);
  if (kind === "boolean") return value ? "true" : "false";
  if (kind === "number") {
    return value !== value || value === Infinity || value === -Infinity ? "null" : String(value);
  }
  if (kind === "undefined" || kind === "function" || kind === "symbol") {
    return arrayElement
      ? "{" + __v8xJsonQuote(String.fromCharCode(0) + "v8x-undefined") + ":true}"
      : undefined;
  }
  if (Array.isArray(value)) {
    let result = "[";
    for (let index = 0; index < value.length; index++) {
      if (index !== 0) result += ",";
      result += __v8xEncodeEvalValue(value[index], true);
    }
    return result + "]";
  }
  let result = "{";
  let first = true;
  for (const key in value) {
    const encoded = __v8xEncodeEvalValue(value[key]);
    if (encoded === undefined) continue;
    if (!first) result += ",";
    first = false;
    result += __v8xJsonQuote(key) + ":" + encoded;
  }
  return result + "}";
}
function __v8xRuntimeEvalJson(source: any, globalObject: any): string {
  try {
    let rewritten = "";
    let index = 0;
    let quote = 0;
    let escaped = false;
    let lineComment = false;
    let blockComment = false;
    while (index < source.length) {
      const code = source.charCodeAt(index);
      const nextCode = index + 1 < source.length
        ? source.charCodeAt(index + 1)
        : -1;
      if (lineComment) {
        rewritten += String.fromCharCode(code);
        index += 1;
        if (code === 10 || code === 13) lineComment = false;
        continue;
      }
      if (blockComment) {
        rewritten += String.fromCharCode(code);
        index += 1;
        if (code === 42 && nextCode === 47) {
          rewritten += "/";
          index += 1;
          blockComment = false;
        }
        continue;
      }
      if (quote !== 0) {
        rewritten += String.fromCharCode(code);
        index += 1;
        if (escaped) escaped = false;
        else if (code === 92) escaped = true;
        else if (code === quote) quote = 0;
        continue;
      }
      if (code === 39 || code === 34 || code === 96) {
        quote = code;
        rewritten += String.fromCharCode(code);
        index += 1;
        continue;
      }
      if (code === 47 && nextCode === 47) {
        lineComment = true;
        rewritten += "//";
        index += 2;
        continue;
      }
      if (code === 47 && nextCode === 42) {
        blockComment = true;
        rewritten += "/*";
        index += 2;
        continue;
      }
      if (source.substring(index, index + 4) === "new ") {
        const constructorStart = index + 4;
        const constructor = source.substring(constructorStart);
        let constructorLength = 0;
        if (constructor.substring(0, 11) === "Uint8Array(") {
          constructorLength = 10;
        } else if (constructor.substring(0, 12) === "Uint16Array(") {
          constructorLength = 11;
        } else if (constructor.substring(0, 12) === "Uint32Array(") {
          constructorLength = 11;
        } else if (constructor.substring(0, 11) === "Int32Array(") {
          constructorLength = 10;
        } else if (constructor.substring(0, 15) === "BigUint64Array(") {
          constructorLength = 14;
        } else if (constructor.substring(0, 14) === "BigInt64Array(") {
          constructorLength = 13;
        }
        if (constructorLength !== 0) {
          index = constructorStart + constructorLength;
          continue;
        }
      }
      if (source.substring(index, index + 10) === ".subarray(") {
        index += 10;
        while (index < source.length && source.charCodeAt(index) !== 41) {
          index += 1;
        }
        if (index < source.length) index += 1;
        continue;
      }
      const start = index;
      let sign = "";
      if (source.charCodeAt(index) === 45 && index + 1 < source.length) {
        const next = source.charCodeAt(index + 1);
        if (next >= 48 && next <= 57) { sign = "-"; index += 1; }
      }
      const first = source.charCodeAt(index);
      if (first >= 48 && first <= 57) {
        const digitsStart = index;
        while (index < source.length) {
          const code = source.charCodeAt(index);
          if (code < 48 || code > 57) break;
          index += 1;
        }
        if (index < source.length && source.charCodeAt(index) === 110) {
          const marker = String.fromCharCode(0) + "v8x-bigint:" + sign +
            source.substring(digitsStart, index);
          rewritten += __v8xJsonQuote(marker);
          index += 1;
          continue;
        }
      }
      index = start;
      rewritten += String.fromCharCode(source.charCodeAt(index));
      index += 1;
    }
    const value = executeIndirectEval(parse, rewritten, globalObject);
    exposeRuntimeEvalGlobalLexicalCells(globalObject);
    exposeRuntimeEvalObject(globalObject);
    if (value === undefined) return "U";
    const encoded = __v8xEncodeEvalValue(value);
    return encoded === undefined ? "U" : "J" + encoded;
  } catch (error) {
    exposeRuntimeEvalGlobalLexicalCells(globalObject);
    exposeRuntimeEvalObject(globalObject);
    const name = error && (error as any).name || "Error";
    const message = error && (error as any).message || String(error);
    return "E{" + __v8xJsonQuote("name") + ":" + __v8xJsonQuote(name) +
      "," + __v8xJsonQuote("message") + ":" + __v8xJsonQuote(message) + "}";
  }
}
export function __runtime_indirect_eval(source: any, globalObject: any): any {
  const encoded = __v8xRuntimeEvalJson(source, globalObject);
  const packet: any[] = [];
  for (let index = 0; index < encoded.length; index++) {
    packet.push(encoded.charCodeAt(index));
  }
  return runtimeEvalResult(true, packet);
}
export function __v8x_runtime_eval_json(source: string, globalObject: any): string {
  return __v8xRuntimeEvalJson(source, globalObject);
}
`;
    const providerSource = provider
      .buildRuntimeEvalProviderSource()
      .replace(
        "export function __runtime_indirect_eval(",
        "function __v8x_runtime_indirect_eval_carrier(",
      );
    const providerResult = await compiler.compile(
      providerSource + providerBridge,
      {
      ...provider.RUNTIME_EVAL_PROVIDER_COMPILE_OPTIONS,
      },
    );
    const providerBinary = optimizeBinary(
      wasmOpt,
      checkedCompile(providerResult, "runtime-eval provider"),
      "runtime-eval provider",
    );
    const providerModule = new WebAssembly.Module(providerBinary);
    const providerImports = WebAssembly.Module.imports(providerModule);
    if (providerImports.length !== 0) {
      throw new Error(`runtime-eval provider has ${providerImports.length} imports, expected zero`);
    }
    mkdirSync(dirname(providerOutput), { recursive: true });
    writeFileSync(providerOutput, providerBinary);
  }

  console.log(JSON.stringify({
    denoRef: actualDenoRef,
    artifact: output,
    artifactBytes: appBinary.length,
    fixtures,
    provider: providerOutput,
  }, null, 2));
}

main().catch((error) => {
  console.error(error?.stack ?? error);
  process.exitCode = 1;
});
