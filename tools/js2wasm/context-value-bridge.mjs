// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
//
// Numeric handles keep WasmGC values in their owning realm. Rust does not
// reinterpret GC layouts or copy object graphs. Handles are currently strong
// roots until the context is destroyed; per-handle release is not implemented.
export const CONTEXT_VALUE_BRIDGE_SOURCE = `
declare function __v8x_host_call(id: number, receiver: number, args: number): number;
const __v8xValues: any[] = [undefined, globalThis];
const __v8xValueIds = new Map<any, number>();
__v8xValueIds.set(undefined, 0);
__v8xValueIds.set(globalThis, 1);
// Map uses SameValueZero. The bridge deliberately distinguishes signed zero.
let __v8xNegativeZeroId = -1;
function __v8xValueAt(id: number): any {
  if (id < 0 || id !== Math.floor(id) || id >= __v8xValues.length)
    throw new RangeError("invalid realm value handle");
  return __v8xValues[id];
}
function __v8xKeepValue(value: any): number {
  const negativeZero = typeof value === "number" && value === 0 && 1 / value < 0;
  if (negativeZero && __v8xNegativeZeroId >= 0) return __v8xNegativeZeroId;
  if (!negativeZero) {
    const previous = __v8xValueIds.get(value);
    if (previous !== undefined) return previous;
  }
  __v8xValues.push(value);
  const id = __v8xValues.length - 1;
  if (negativeZero) __v8xNegativeZeroId = id;
  else __v8xValueIds.set(value, id);
  return id;
}
// Compiler-owned seam: identity in a single module, a canonical callable
// adapter when the runtime-eval provider can observe the host closure.
function __runtime_eval_wrap_aot_callable(value: any): any { return value; }
export function __v8x_value_host_function(id: number): number {
  return __v8xKeepValue(__runtime_eval_wrap_aot_callable(function(this: any, ...args: any[]): any {
    if (new.target) throw new TypeError("constructing a host callback is not implemented");
    const result = __v8x_host_call(id, __v8xKeepValue(this), __v8xKeepValue(args));
    if (result < 0) throw __v8xValueAt(-result - 1);
    return __v8xValueAt(result);
  }));
}
export function __v8x_value_error(name: number, message: number): number {
  const kind = __v8xValueAt(name);
  const text = __v8xValueAt(message);
  if (kind === "TypeError") return __v8xKeepValue(new TypeError(text));
  if (kind === "RangeError") return __v8xKeepValue(new RangeError(text));
  return __v8xKeepValue(new Error(text));
}
export function __v8x_value_symbol_create(kind: number, text: number): number {
  const description = __v8xValueAt(text);
  if (kind === 2) return __v8xKeepValue(Symbol.iterator);
  if (kind === 1) {
    if (typeof description !== "string") throw new TypeError("expected Symbol registry key");
    return __v8xKeepValue(Symbol.for(description));
  }
  if (kind !== 0 || (description !== undefined && typeof description !== "string"))
    throw new TypeError("invalid Symbol construction");
  return __v8xKeepValue(description === undefined ? Symbol() : Symbol(description));
}
export function __v8x_value_symbol_kind(id: number): number {
  const value = __v8xValueAt(id);
  if (typeof value !== "symbol") throw new TypeError("expected Symbol handle");
  if (value === Symbol.iterator) return 2;
  return Symbol.keyFor(value) === undefined ? 0 : 1;
}
export function __v8x_value_symbol_text(id: number): number {
  const value = __v8xValueAt(id);
  if (typeof value !== "symbol") throw new TypeError("expected Symbol handle");
  const key = Symbol.keyFor(value);
  return __v8xKeepValue(key === undefined ? value.description : key);
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

export function __v8x_value_to_number(id: number): number {
  const value = __v8xValueAt(id);
  // Unary plus performs ToNumber, unlike Number() which accepts BigInt.
  // Keep the thrown value itself so native TryCatch retains its identity.
  try { return __v8xKeepValue([true, +value]); }
  catch (error) { return __v8xKeepValue([false, error]); }
}

const __v8xHostBufferIds = new Set<number>();
const __v8xPacketIds = new Set<number>();
// Native-only, unpublished, one-shot packets do not need canonical identity.
// Keep them rooted, but avoid the object-key bucket scan on insert/delete.
export function __v8x_value_packet_create(length: number): number {
  if (length < 0 || length > 2147483647 || length !== Math.floor(length))
    throw new RangeError("invalid transfer packet length");
  const id = __v8xValues.length;
  __v8xValues.push(new ArrayBuffer(length));
  __v8xHostBufferIds.add(id);
  __v8xPacketIds.add(id);
  return id;
}
function __v8xRetirePacket(id: number): void {
  __v8xHostBufferIds.delete(id);
  // Legacy callers may still consume a canonical buffer as a packet.
  if (!__v8xPacketIds.delete(id)) __v8xValueIds.delete(__v8xValues[id]);
  __v8xValues[id] = undefined;
}
export function __v8x_value_buffer_create(length: number): number {
  if (length < 0 || length > 2147483647 || length !== Math.floor(length))
    throw new RangeError("invalid host buffer length");
  const id = __v8xKeepValue(new ArrayBuffer(length));
  __v8xHostBufferIds.add(id);
  return id;
}
// Private storage ABI: only fixed buffers created by the bridge are accepted.
// Rust validates the native byte-vector layout through Wasmtime GC APIs.
export function __v8x_value_buffer_storage(id: number): any {
  if (!__v8xHostBufferIds.has(id)) throw new TypeError("not a host-backed buffer handle");
  return __v8xValueAt(id);
}
// Private transient transfer packets. Decode UTF-16 explicitly, including lone
// surrogates, rather than passing through a lossy UTF-8 conversion.
export function __v8x_value_string_from_buffer(id: number): number {
  const bytes = new Uint8Array(__v8x_value_buffer_storage(id));
  if (bytes.length % 2 !== 0) throw new RangeError("invalid UTF-16 packet");
  let text = "";
  for (let i = 0; i < bytes.length; i += 2)
    text += String.fromCharCode(bytes[i] + bytes[i + 1] * 256);
  __v8xRetirePacket(id);
  return __v8xKeepValue(text);
}
export function __v8x_value_define_packet(owner: number, packet: number): void {
  const bytes = new Uint8Array(__v8x_value_buffer_storage(packet));
  if (bytes.length % 12 !== 0) throw new RangeError("invalid property packet");
  for (let i = 0; i < bytes.length; i += 12) {
    const key = bytes[i] + bytes[i+1]*256 + bytes[i+2]*65536 + bytes[i+3]*16777216;
    const value = bytes[i+4] + bytes[i+5]*256 + bytes[i+6]*65536 + bytes[i+7]*16777216;
    const flags = bytes[i+8] + bytes[i+9]*256 + bytes[i+10]*65536 + bytes[i+11]*16777216;
    __v8x_value_define_data(owner, key, value, flags);
  }
  __v8xRetirePacket(packet);
}
export function __v8x_value_typed_array(buffer: number, kind: number, offset: number, length: number): number {
  const value = __v8x_value_buffer_storage(buffer);
  if (offset < 0 || offset !== Math.floor(offset) || length < 0 || length !== Math.floor(length))
    throw new RangeError("invalid host view range");
  if (kind === 0) return __v8xKeepValue(new Uint8Array(value, offset, length));
  if (kind === 1) return __v8xKeepValue(new Uint16Array(value, offset, length));
  if (kind === 2) return __v8xKeepValue(new Uint32Array(value, offset, length));
  if (kind === 3) return __v8xKeepValue(new Int32Array(value, offset, length));
  if (kind === 4) return __v8xKeepValue(new BigUint64Array(value, offset, length));
  if (kind === 5) return __v8xKeepValue(new BigInt64Array(value, offset, length));
  throw new RangeError("invalid host view kind");
}
export function __v8x_value_object(): number { return __v8xKeepValue({}); }
export function __v8x_value_array(): number { return __v8xKeepValue([]); }
export function __v8x_value_get_prototype(owner: number): number {
  return __v8xKeepValue(Object.getPrototypeOf(__v8xValueAt(owner)));
}
export function __v8x_value_set_prototype(owner: number, prototype: number): number {
  return Reflect.setPrototypeOf(__v8xValueAt(owner), __v8xValueAt(prototype)) ? 1 : 0;
}
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
  "__v8x_value_symbol_create",
  "__v8x_value_symbol_kind",
  "__v8x_value_symbol_text",
  "__v8x_value_kind",
  "__v8x_value_null",
  "__v8x_value_boolean",
  "__v8x_value_as_boolean",
  "__v8x_value_number",
  "__v8x_value_as_number",
  "__v8x_value_to_number",
  "__v8x_value_buffer_create",
  "__v8x_value_packet_create",
  "__v8x_value_buffer_storage",
  "__v8x_value_string_from_buffer",
  "__v8x_value_define_packet",
  "__v8x_value_typed_array",
  "__v8x_value_object",
  "__v8x_value_array",
  "__v8x_value_get_prototype",
  "__v8x_value_set_prototype",
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
    /export function (__v8x_value_\w+)\(([^)]*)\): (number|void|any) \{/g
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
