// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
//
// Numeric handles keep WasmGC values in their owning realm. Rust does not
// reinterpret GC layouts or copy object graphs. Handles are currently strong
// roots until the context is destroyed; per-handle release is not implemented.
export const CONTEXT_VALUE_BRIDGE_SOURCE = `
declare function __v8x_host_call(id: number, receiver: number, args: number): number;
const __v8xValues: any[] = [undefined, globalThis];
function __v8xValueAt(id: number): any {
  if (id < 0 || id !== Math.floor(id) || id >= __v8xValues.length)
    throw new RangeError("invalid realm value handle");
  return __v8xValues[id];
}
function __v8xKeepValue(value: any): number {
  for (let i = 0; i < __v8xValues.length; i++) {
    const previous = __v8xValues[i];
    if (previous === value) {
      if (typeof value !== "number" || value !== 0 || 1 / previous === 1 / value) return i;
    } else if (typeof previous === "number" && typeof value === "number"
      && previous !== previous && value !== value) return i;
  }
  __v8xValues.push(value);
  return __v8xValues.length - 1;
}
export function __v8x_value_host_function(id: number): number {
  return __v8xKeepValue(function(this: any, ...args: any[]): any {
    if (new.target) throw new TypeError("constructing a host callback is not implemented");
    const result = __v8x_host_call(id, __v8xKeepValue(this), __v8xKeepValue(args));
    if (result < 0) throw __v8xValueAt(-result - 1);
    return __v8xValueAt(result);
  });
}
export function __v8x_value_error(name: number, message: number): number {
  const kind = __v8xValueAt(name);
  const text = __v8xValueAt(message);
  if (kind === "TypeError") return __v8xKeepValue(new TypeError(text));
  if (kind === "RangeError") return __v8xKeepValue(new RangeError(text));
  return __v8xKeepValue(new Error(text));
}
export function __v8x_value_global(): number { return 1; }
export function __v8x_value_kind(id: number): number {
  const value = __v8xValueAt(id);
  if (value === undefined) return 0;
  if (value === null) return 1;
  const type = typeof value;
  if (type === "boolean") return 2;
  if (type === "number") return 3;
  if (type === "string") return 4;
  if (type === "function") return 6;
  if (type === "bigint") return 7;
  if (type === "symbol") return 8;
  if (Array.isArray(value)) return 9;
  return 5;
}
export function __v8x_value_null(): number { return __v8xKeepValue(null); }
export function __v8x_value_boolean(value: number): number {
  if (value !== 0 && value !== 1) throw new RangeError("invalid boolean encoding");
  return __v8xKeepValue(value === 1);
}
export function __v8x_value_as_boolean(id: number): number {
  const value = __v8xValueAt(id);
  if (typeof value !== "boolean") throw new TypeError("expected boolean handle");
  return value ? 1 : 0;
}
export function __v8x_value_number(value: number): number { return __v8xKeepValue(value); }
export function __v8x_value_as_number(id: number): number {
  const value = __v8xValueAt(id);
  if (typeof value !== "number") throw new TypeError("expected number handle");
  return value;
}
export function __v8x_value_object(): number { return __v8xKeepValue({}); }
export function __v8x_value_array(): number { return __v8xKeepValue([]); }
export function __v8x_value_get(owner: number, key: number): number {
  return __v8xKeepValue(__v8xValueAt(owner)[__v8xValueAt(key)]);
}
export function __v8x_value_set(owner: number, key: number, value: number): void {
  __v8xValueAt(owner)[__v8xValueAt(key)] = __v8xValueAt(value);
}
export function __v8x_value_define_data(owner: number, key: number, value: number, flags: number): void {
  if (flags < 0 || flags > 7 || flags !== Math.floor(flags))
    throw new RangeError("invalid property attributes");
  Object.defineProperty(__v8xValueAt(owner), __v8xValueAt(key), {
    value: __v8xValueAt(value), writable: (flags & 1) === 0,
    enumerable: (flags & 2) === 0, configurable: (flags & 4) === 0,
  });
}
export function __v8x_value_call(callable: number, receiver: number, args: number): number {
  return __v8xKeepValue(__v8xValueAt(callable).apply(__v8xValueAt(receiver), __v8xValueAt(args)));
}
export function __v8x_value_utf16_length(id: number): number {
  const value = __v8xValueAt(id);
  if (typeof value !== "string") throw new TypeError("expected string handle");
  return value.length;
}
export function __v8x_value_utf16_unit(id: number, index: number): number {
  const value = __v8xValueAt(id);
  if (typeof value !== "string") throw new TypeError("expected string handle");
  return value.charCodeAt(index);
}
export function __v8x_value_string_empty(): number { return __v8xKeepValue(""); }
export function __v8x_value_string_append(id: number, unit: number): number {
  const value = __v8xValueAt(id);
  if (typeof value !== "string") throw new TypeError("expected string handle");
  return __v8xKeepValue(value + String.fromCharCode(unit));
}
`;

export const CONTEXT_VALUE_BRIDGE_EXPORTS = Object.freeze([
  "__v8x_value_global",
  "__v8x_value_host_function",
  "__v8x_value_error",
  "__v8x_value_kind",
  "__v8x_value_null",
  "__v8x_value_boolean",
  "__v8x_value_as_boolean",
  "__v8x_value_number",
  "__v8x_value_as_number",
  "__v8x_value_object",
  "__v8x_value_array",
  "__v8x_value_get",
  "__v8x_value_set",
  "__v8x_value_define_data",
  "__v8x_value_call",
  "__v8x_value_utf16_length",
  "__v8x_value_utf16_unit",
  "__v8x_value_string_empty",
  "__v8x_value_string_append",
]);

export function contextValueBridgeEntrypoints(modulePath) {
  const signatures = [...CONTEXT_VALUE_BRIDGE_SOURCE.matchAll(
    /export function (__v8x_value_\w+)\(([^)]*)\): (number|void) \{/g
  )];
  if (signatures.length !== CONTEXT_VALUE_BRIDGE_EXPORTS.length ||
      CONTEXT_VALUE_BRIDGE_EXPORTS.some(name => !signatures.some(s => s[1] === name))) {
    throw new Error("context bridge entrypoint signatures are incomplete");
  }
  return signatures.map(([,name,parameters,result]) => {
    const args = parameters ? parameters.split(",").map(p => {
      const match = /^\s*(\w+): number\s*$/.exec(p);
      if (!match) throw new Error("unsupported context bridge parameter: " + p);
      return match[1];
    }).join(", ") : "";
    return `import { ${name} as imported${name} } from ${JSON.stringify(modulePath)};\n` +
      `export function ${name}(${parameters}): ${result} { ${result === "void" ? "" : "return "}imported${name}(${args}); }\n`;
  }).join("\n");
}
