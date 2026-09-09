#![cfg(feature = "js2wasm_runtime_compile")]

use wasmtime::{
  AnyRef, ArrayRef, CallHook, Caller, Config, Engine, Linker, Module,
  OwnedRooted, Store, Val,
};

#[derive(Default)]
struct State {
  host: Vec<u8>,
  bytes: Option<OwnedRooted<ArrayRef>>,
  observations: Vec<u8>,
  view: f64,
  key: f64,
  fail_host: bool,
}

fn call(
  store: &mut Store<State>,
  instance: wasmtime::Instance,
  name: &str,
  args: &[f64],
) -> f64 {
  let f = instance.get_func(&mut *store, name).unwrap();
  let params: Vec<_> = args.iter().map(|v| Val::F64(v.to_bits())).collect();
  let mut result = [Val::F64(0)];
  f.call(store, &params, &mut result).unwrap();
  result[0].f64().unwrap()
}

fn string(
  store: &mut Store<State>,
  instance: wasmtime::Instance,
  text: &str,
) -> f64 {
  let mut id = call(store, instance, "__v8x_value_string_empty", &[]);
  for unit in text.encode_utf16() {
    id = call(
      store,
      instance,
      "__v8x_value_string_append",
      &[id, unit as f64],
    );
  }
  id
}

#[test]
#[ignore = "requires the compiled context-value fixture"]
fn synchronizes_gc_bytes_across_reentry_and_wasm_exception() {
  let mut config = Config::new();
  config
    .wasm_gc(true)
    .wasm_function_references(true)
    .wasm_tail_call(true)
    .wasm_exceptions(true);
  let engine = Engine::new(&config).unwrap();
  let path = std::env::var("V8X_JS2WASM_CONTEXT_VALUES_WASM").unwrap();
  let module =
    Module::from_binary(&engine, &std::fs::read(path).unwrap()).unwrap();
  let mut linker = Linker::new(&engine);
  linker
    .func_wrap(
      "v8x:deno",
      "__v8x_host_call",
      |mut caller: Caller<'_, State>,
       _id: f64,
       _this: f64,
       _args: f64|
       -> wasmtime::Result<f64> {
        let byte = caller.data().host[0];
        caller.data_mut().observations.push(byte);
        caller.data_mut().host[0] = 9;
        let view = caller.data().view;
        let key = caller.data().key;
        let get = caller
          .get_export("__v8x_value_get")
          .unwrap()
          .into_func()
          .unwrap()
          .typed::<(f64, f64), f64>(&caller)?;
        let value = get.call(&mut caller, (view, key))?;
        let number = caller
          .get_export("__v8x_value_as_number")
          .unwrap()
          .into_func()
          .unwrap()
          .typed::<f64, f64>(&caller)?;
        assert_eq!(number.call(&mut caller, value)?, 9.0);
        caller.data_mut().host[0] = 15;
        if caller.data().fail_host {
          return Err(wasmtime::Error::msg("requested host failure"));
        }
        Ok(0.0)
      },
    )
    .unwrap();
  let mut store = Store::new(&engine, State::default());
  let instance = linker.instantiate(&mut store, &module).unwrap();
  let buffer = call(&mut store, instance, "__v8x_value_buffer_create", &[16.0]);
  let view = call(
    &mut store,
    instance,
    "__v8x_value_typed_array",
    &[buffer, 0.0, 0.0, 16.0],
  );
  let storage = instance
    .get_func(&mut store, "__v8x_value_buffer_storage")
    .unwrap();
  let mut raw = [Val::ExternRef(None)];
  storage
    .call(&mut store, &[Val::F64(buffer.to_bits())], &mut raw)
    .unwrap();
  let value =
    AnyRef::convert_extern(&mut store, *raw[0].unwrap_externref().unwrap())
      .unwrap();
  let vector = value.as_struct(&store).unwrap().unwrap();
  assert_eq!(vector.field(&mut store, 0).unwrap().i32(), Some(16));
  let data = vector.field(&mut store, 1).unwrap();
  let bytes = data
    .unwrap_anyref()
    .unwrap()
    .as_array(&store)
    .unwrap()
    .unwrap();
  assert_eq!(bytes.len(&store).unwrap(), 16);
  assert!(matches!(
    bytes.ty(&store).unwrap().element_type(),
    wasmtime::StorageType::I8
  ));
  let owned = bytes.to_owned_rooted(&mut store).unwrap();
  let key = string(&mut store, instance, "0");
  let global = call(&mut store, instance, "__v8x_value_global", &[]);
  let name = string(&mut store, instance, "exerciseSharedBuffer");
  let function = call(&mut store, instance, "__v8x_value_get", &[global, name]);
  let throw_name = string(&mut store, instance, "throwSharedBuffer");
  let throw_function = call(
    &mut store,
    instance,
    "__v8x_value_get",
    &[global, throw_name],
  );
  let host = call(&mut store, instance, "__v8x_value_host_function", &[0.0]);
  let args = call(&mut store, instance, "__v8x_value_array", &[]);
  let set = instance
    .get_typed_func::<(f64, f64, f64), ()>(&mut store, "__v8x_value_set")
    .unwrap();
  set.call(&mut store, (args, key, host)).unwrap();
  let one = string(&mut store, instance, "1");
  set.call(&mut store, (args, one, view)).unwrap();
  let throw_args = call(&mut store, instance, "__v8x_value_array", &[]);
  set.call(&mut store, (throw_args, key, view)).unwrap();
  store.data_mut().bytes = Some(owned);
  store.data_mut().host = vec![1; 16];
  store.data_mut().view = view;
  store.data_mut().key = key;
  store.call_hook(|mut context, event| {
    let binding = context.data_mut().bytes.take().unwrap();
    let bytes = binding.to_rooted(&mut context);
    let result = (|| -> wasmtime::Result<()> {
      if event.exiting_host() {
        let source = context.data().host.clone();
        for (i, byte) in source.into_iter().enumerate() {
          bytes.set(&mut context, i as u32, Val::I32(byte as i32))?;
        }
      } else if matches!(
        event,
        CallHook::CallingHost | CallHook::ReturningFromWasm
      ) {
        let mut output = Vec::new();
        for i in 0..bytes.len(&context)? {
          output.push(bytes.get(&mut context, i)?.i32().unwrap() as u8);
        }
        context.data_mut().host = output;
      }
      Ok(())
    })();
    context.data_mut().bytes = Some(binding);
    result
  });
  let result = call(
    &mut store,
    instance,
    "__v8x_value_call",
    &[function, global, args],
  );
  assert_eq!(
    call(&mut store, instance, "__v8x_value_as_number", &[result]),
    15.0
  );
  assert_eq!(store.data().observations, vec![7]);
  assert_eq!(store.data().host[0], 15);
  let invoke = instance
    .get_typed_func::<(f64, f64, f64), f64>(&mut store, "__v8x_value_call")
    .unwrap();
  store.data_mut().fail_host = true;
  let error = invoke
    .call(&mut store, (function, global, args))
    .unwrap_err();
  assert!(format!("{error:#}").contains("requested host failure"));
  assert_eq!(store.data().host[0], 15);
  assert!(!store.has_pending_exception());
  store.data_mut().fail_host = false;
  assert!(
    invoke
      .call(&mut store, (throw_function, global, throw_args))
      .is_err()
  );
  assert_eq!(store.data().host[0], 11);
  assert!(
    store.has_pending_exception(),
    "buffer synchronization must preserve the thrown Wasm value"
  );
}
