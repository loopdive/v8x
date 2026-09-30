// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
// Wrap the pinned classic scripts without changing their bodies. Deno's Rust
// host chooses when each script runs, including the native-op registration gap.
export const CORE_SCRIPT_ORDER = [
  "00_primordials.js", "00_infra.js", "02_timers.js", "01_core.js",
];

export function stagedCoreSource(sources, { nativeNamespace = false } = {}) {
  const scripts = CORE_SCRIPT_ORDER.map((path, index) => {
    const source = sources.get(path);
    if (typeof source !== "string") throw new Error("missing core script: " + path);
    return "function script" + index + "(): void {\n" + source + "\n}";
  });
  const moduleSource = sources.get("mod.js");
  const exports = "export { core, internals, primordials };";
  if (typeof moduleSource !== "string" || moduleSource.split(exports).length !== 2) {
    throw new Error("unsupported core module export shape");
  }
  const imports = nativeNamespace
    ? 'import { initializeCoreNamespace } from "./core-bindings.ts";\nimport * as coreNamespace from "./core-namespace.ts";\n'
    : "";
  const publication = nativeNamespace
    ? "initializeCoreNamespace(core, internals, primordials); return coreNamespace;"
    : "return { core, internals, primordials };";
  return imports + scripts.join("\n") + `
let phase = 0;
let moduleStarted = false;
export function scriptPhase(): number { return phase; }
export function runScript(index: number): number {
  if (index !== phase || index < 0 || index > 3) throw new Error("Deno script order mismatch");
  // A failed script cannot be resumed as though its partial writes never ran.
  phase = -1;
  if (index === 0) script0();
  else if (index === 1) script1();
  else if (index === 2) script2();
  else script3();
  phase = index + 1;
  return phase;
}
export function runModule(): any {
  if (phase !== 4 || moduleStarted) throw new Error("Deno module order mismatch");
  moduleStarted = true;
  ` + moduleSource.replace(exports, publication) + "\n}\n";
}

// The public namespace re-exports only the original three names. Its private
// initialization function lives in a separate module, so it cannot leak into
// Reflect.ownKeys on the public namespace. Bindings remain compiler-native.
export function stagedCoreNamespaceSources() {
  return {
    "core-bindings.ts": `
export let core: any;
export let internals: any;
export let primordials: any;
let initialized = false;
export function initializeCoreNamespace(c: any, i: any, p: any): void {
  if (initialized) throw new Error("Deno core namespace already initialized");
  core = c; internals = i; primordials = p;
  initialized = true;
}
`,
    "core-namespace.ts": 'export { core, internals, primordials } from "./core-bindings.ts";\n',
  };
}
