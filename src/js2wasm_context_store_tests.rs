use super::*;

// A mutable i32 global, exported as "value". No external compiler or Deno
// artifact is needed to test the store ownership contract.
const GLOBAL_MODULE: &[u8] =
  b"\0asm\x01\0\0\0\x06\x06\x01\x7f\x01\x41\x2a\x0b\x07\x09\x01\x05value\x03\0";
const TRAPPING_INIT: &[u8] = b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x02\x01\0\x07\x11\x01\x0d__module_init\0\0\x0a\x05\x01\x03\0\0\x0b";

fn prepared(shared: &SharedDenoRuntime, bytes: &[u8]) -> PreparedModule {
  let module = Module::new(&shared.engine, bytes).unwrap();
  shared.prepare_module(&module).unwrap()
}

pub fn graphs_share_store_without_replacing_primary_instance() {
  context_imports_require_real_provider_exports();
  let shared = SharedDenoRuntime::new().unwrap();
  let module = prepared(&shared, GLOBAL_MODULE);
  let mut runtime =
    DenoRuntime::instantiate(&shared, &module, PathBuf::from("."), 0).unwrap();
  let primary = runtime
    .instance
    .get_global(&mut runtime.store, "value")
    .unwrap();
  primary
    .set(&mut runtime.store, wasmtime::Val::I32(7))
    .unwrap();

  runtime.instantiate_graph(&shared, &module).unwrap();
  assert_eq!(runtime.graph_instances.len(), 1);
  let graph = runtime.graph_instances[0]
    .get_global(&mut runtime.store, "value")
    .unwrap();
  graph
    .set(&mut runtime.store, wasmtime::Val::I32(9))
    .unwrap();
  let restored = runtime
    .instance
    .get_global(&mut runtime.store, "value")
    .unwrap();
  assert_eq!(restored.get(&mut runtime.store).i32(), Some(7));
  assert_eq!(graph.get(&mut runtime.store).i32(), Some(9));

  let mut other =
    DenoRuntime::instantiate(&shared, &module, PathBuf::from("."), 0).unwrap();
  let isolated = other
    .instance
    .get_global(&mut other.store, "value")
    .unwrap();
  assert_eq!(isolated.get(&mut other.store).i32(), Some(42));
}

pub fn failed_graph_initialization_preserves_primary_and_retains_graph() {
  first_initializer_failure_keeps_published_owner();
  let shared = SharedDenoRuntime::new().unwrap();
  let module = prepared(&shared, GLOBAL_MODULE);
  let mut runtime =
    DenoRuntime::instantiate(&shared, &module, PathBuf::from("."), 0).unwrap();
  let failing = prepared(&shared, TRAPPING_INIT);
  let error = runtime.instantiate_graph(&shared, &failing).unwrap_err();
  assert!(error.contains("__module_init"), "{error}");
  assert_eq!(runtime.graph_instances.len(), 1);
  let primary = runtime
    .instance
    .get_global(&mut runtime.store, "value")
    .unwrap();
  assert_eq!(primary.get(&mut runtime.store).i32(), Some(42));
  runtime.instantiate_graph(&shared, &module).unwrap();
  assert_eq!(runtime.graph_instances.len(), 2);

  // Exercise failure inside Wasmtime instantiation, before a graph instance
  // exists. Use a tiny cached provider to isolate lifetime from its JS ABI.
  *shared.runtime_eval_provider.lock().unwrap() =
    Some(Module::new(&shared.engine, GLOBAL_MODULE).unwrap());
  let start_trap = b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x02\x01\0\x08\x01\0\x0a\x05\x01\x03\0\0\x0b";
  let failing = PreparedModule::RuntimeEval(
    Module::new(&shared.engine, start_trap).unwrap(),
  );
  let error = runtime.instantiate_graph(&shared, &failing).unwrap_err();
  assert!(error.contains("instantiate js2wasm artifact"), "{error}");
  let provider = runtime._runtime_eval_provider.expect("provider retained");
  let value = provider.get_global(&mut runtime.store, "value").unwrap();
  value
    .set(&mut runtime.store, wasmtime::Val::I32(99))
    .unwrap();

  let good = PreparedModule::RuntimeEval(
    Module::new(&shared.engine, GLOBAL_MODULE).unwrap(),
  );
  runtime.instantiate_graph(&shared, &good).unwrap();
  assert_eq!(
    shared.runtime_eval_instantiations.load(Ordering::Relaxed),
    1
  );
  let reused = runtime
    ._runtime_eval_provider
    .unwrap()
    .get_global(&mut runtime.store, "value")
    .unwrap();
  assert_eq!(reused.get(&mut runtime.store).i32(), Some(99));
}

fn section(bytes: &mut Vec<u8>, id: u8, payload: &[u8]) {
  assert!(payload.len() < 128);
  bytes.extend_from_slice(&[id, payload.len() as u8]);
  bytes.extend_from_slice(payload);
}

fn context_fixture(provider: bool, name: &str) -> Vec<u8> {
  let mut bytes = b"\0asm\x01\0\0\0".to_vec();
  section(&mut bytes, 1, &[2, 0x60, 0, 1, 0x6f, 0x60, 0, 0]);
  if provider {
    section(&mut bytes, 3, &[1, 0]);
    let mut exports = vec![1, name.len() as u8];
    exports.extend_from_slice(name.as_bytes());
    exports.extend_from_slice(&[0, 0]);
    section(&mut bytes, 7, &exports);
    section(&mut bytes, 10, &[1, 4, 0, 0xd0, 0x6f, 0x0b]);
  } else {
    let mut imports = vec![1, CONTEXT_IMPORT_MODULE.len() as u8];
    imports.extend_from_slice(CONTEXT_IMPORT_MODULE.as_bytes());
    imports.push(name.len() as u8);
    imports.extend_from_slice(name.as_bytes());
    imports.extend_from_slice(&[0, 0]);
    section(&mut bytes, 2, &imports);
    section(&mut bytes, 3, &[1, 1]);
    let mut exports = vec![1, 13];
    exports.extend_from_slice(b"__module_init");
    exports.extend_from_slice(&[0, 1]);
    section(&mut bytes, 7, &exports);
    section(&mut bytes, 10, &[1, 5, 0, 0x10, 0, 0x1a, 0x0b]);
  }
  bytes
}

// This verifies linking only. A null fixture is deliberately NOT evidence
// that a compiled realm is synchronized with a Rust Context global.
fn context_imports_require_real_provider_exports() {
  let shared = SharedDenoRuntime::new().unwrap();
  let name = CONTEXT_IMPORTS[0];
  let consumer = prepared(&shared, &context_fixture(false, name));
  let primary = prepared(&shared, GLOBAL_MODULE);
  let mut runtime =
    DenoRuntime::instantiate(&shared, &primary, PathBuf::from("."), 0).unwrap();
  *shared.runtime_eval_provider.lock().unwrap() =
    Some(Module::new(&shared.engine, GLOBAL_MODULE).unwrap());
  let error = runtime.instantiate_graph(&shared, &consumer).unwrap_err();
  assert!(error.contains("missing context export"), "{error}");

  let shared = SharedDenoRuntime::new().unwrap();
  *shared.runtime_eval_provider.lock().unwrap() =
    Some(Module::new(&shared.engine, GLOBAL_MODULE).unwrap());
  let consumer = prepared(&shared, &context_fixture(false, name));
  let primary = prepared(&shared, &context_fixture(true, name));
  let mut runtime =
    DenoRuntime::instantiate(&shared, &primary, PathBuf::from("."), 0).unwrap();
  runtime.instantiate_graph(&shared, &consumer).unwrap();
  runtime.instantiate_graph(&shared, &consumer).unwrap();
  assert_eq!(
    shared.runtime_eval_instantiations.load(Ordering::Relaxed),
    1
  );

  let unknown = Module::new(
    &shared.engine,
    context_fixture(false, "__unknown_context_import"),
  )
  .unwrap();
  let error = match shared.prepare_module(&unknown) {
    Ok(_) => panic!("unknown context import was accepted"),
    Err(error) => error,
  };
  assert!(
    error.contains("unimplemented js2wasm host import"),
    "{error}"
  );
}

fn first_initializer_failure_keeps_published_owner() {
  let shared = SharedDenoRuntime::new().unwrap();
  let mut bytes = b"\0asm\x01\0\0\0".to_vec();
  section(&mut bytes, 1, &[1, 0x60, 0, 0]);
  section(&mut bytes, 3, &[1, 0]);
  section(&mut bytes, 6, &[1, 0x7f, 1, 0x41, 42, 0x0b]);
  let mut exports = vec![2, 5];
  exports.extend_from_slice(b"value");
  exports.extend_from_slice(&[3, 0, 13]);
  exports.extend_from_slice(b"__module_init");
  exports.extend_from_slice(&[0, 0]);
  section(&mut bytes, 7, &exports);
  section(&mut bytes, 10, &[1, 7, 0, 0x41, 9, 0x24, 0, 0, 0x0b]);
  let module = prepared(&shared, &bytes);
  let mut published = None;
  let result = DenoRuntime::instantiate_published(
    &shared,
    &module,
    PathBuf::from("."),
    0,
    |runtime| {
      published = Some(runtime);
      Ok(())
    },
  );
  let error = match result {
    Ok(_) => panic!("first initializer unexpectedly succeeded"),
    Err(error) => error,
  };
  assert!(error.contains("__module_init"), "{error}");
  let retained = published.expect("owner published before initialization");
  let mut owner = retained.borrow_mut();
  let runtime = &mut *owner;
  let value = runtime
    .instance
    .get_global(&mut runtime.store, "value")
    .unwrap();
  assert_eq!(value.get(&mut runtime.store).i32(), Some(9));
  let next = prepared(&shared, GLOBAL_MODULE);
  runtime.instantiate_graph(&shared, &next).unwrap();
  assert_eq!(value.get(&mut runtime.store).i32(), Some(9));
  assert_eq!(shared.instantiations.load(Ordering::Relaxed), 2);
}
