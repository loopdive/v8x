// A narrow implementation of the frozen js2wasm:runtime-eval ABI used to
// verify v8x's cross-module WasmGC identity plumbing. Production supplies the
// full interpreter provider built by js2wasm.

function result(ok: boolean, value: any): any {
  return [ok, value];
}

function refusal(): any {
  return result(false, new TypeError("unsupported fixture runtime eval"));
}

export function __runtime_new_function(
  _paramString: any,
  _bodyString: any,
  _globalObject: any,
): any {
  return refusal();
}

export function __runtime_indirect_eval(source: any, globalObject: any): any {
  if (source === "runtimeCounter = runtimeCounter + 2") {
    globalObject.runtimeCounter = globalObject.runtimeCounter + 2;
    return result(true, globalObject.runtimeCounter);
  }
  if (source === "runtimeCounter") {
    return result(true, globalObject.runtimeCounter);
  }
  return refusal();
}

export function __runtime_direct_eval(
  source: any,
  globalObject: any,
  _thisArg: any,
  _activationState: any,
  _activationSeedNames: any,
  _activationSeedSlots: any,
  _lexicalNames: any,
  _lexicalSlots: any,
  _outerNames: any,
  _outerSlots: any,
  _callerStrict: boolean,
  _mappedParamNames: any,
): any {
  return __runtime_indirect_eval(source, globalObject);
}

export function __runtime_apply_interpreted(
  _callable: any,
  _receiver: any,
  _argc: number,
  _a0: any,
  _a1: any,
  _a2: any,
  _a3: any,
  _a4: any,
  _a5: any,
  _a6: any,
  _a7: any,
): any {
  return refusal();
}
