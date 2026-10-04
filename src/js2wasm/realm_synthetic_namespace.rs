use super::*;

// This carrier is deliberately not activated by ordinary Module evaluation
// yet. Mixed-graph packaging and namespace-exotic reflection remain unfinished.
pub(super) fn bind(
  access: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  module: *const Module,
) -> Result<RealmValue, String> {
  let (namespace, names) = unsafe { module_state(module) }
    .and_then(|state| {
      (state.status == STATUS_EVALUATED).then(|| {
        state.synthetic.as_ref().map(|synthetic| {
          (synthetic.namespace, synthetic.export_names.clone())
        })
      })
    })
    .flatten()
    .ok_or("native namespace requires an evaluated synthetic Module")?;
  if let Some(previous) = binding(namespace) {
    if !Rc::ptr_eq(owner, &previous.runtime) {
      return Err("native namespace belongs to another Context".into());
    }
    return Ok(previous.value);
  }
  if !access.realm_has_export("__v8x_value_module_namespace") {
    return Err("Context lacks native namespace getter transport".into());
  }
  let keys = access.realm_array()?;
  let getters = access.realm_array()?;
  let mut names = names;
  names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
  for (index, name) in names.into_iter().enumerate() {
    let isolate = current_isolate();
    let key = new_string(isolate, name.clone());
    // Private native callback data stays isolate-owned. No pointer or current
    // export value is serialized into Wasm or a build-side source facade.
    let data = allocate::<Value>(
      isolate,
      HeapValue::Array(ArrayState {
        elements: vec![module.cast(), key.cast()],
        properties: Vec::new(),
      }),
    );
    let getter =
      allocate_function(isolate, read_export, data, Vec::new(), ptr::null());
    let getter = host_callbacks::allocate(access, owner, getter)?;
    let key = access.realm_string(&name.encode_utf16().collect::<Vec<_>>())?;
    let index = access
      .realm_string(&index.to_string().encode_utf16().collect::<Vec<_>>())?;
    access.realm_set(keys, index, key)?;
    access.realm_set(getters, index, getter)?;
  }
  let value = access.realm_module_namespace(keys, getters)?;
  // Publish once all accessors exist. Failed construction never adopts the
  // canonical Rust namespace with a partially initialized realm carrier.
  unsafe { isolate_state(current_isolate()) }
    .realm_objects
    .push(RealmObjectBinding {
      host: namespace,
      runtime: owner.clone(),
      value,
    });
  Ok(value)
}

unsafe extern "C" fn read_export(
  info: *const crate::function::FunctionCallbackInfo,
) {
  let result = (|| {
    let data = v8__FunctionCallbackInfo__Data(info);
    let Some(HeapValue::Array(data)) = (unsafe { heap_value(data) }) else {
      return Err("native export getter lost its private binding");
    };
    let [module, key] = data.elements.as_slice() else {
      return Err("native export getter has invalid binding arity");
    };
    let state = unsafe { module_state((*module).cast()) }
      .ok_or("native export getter lost its Module")?;
    let synthetic = state
      .synthetic
      .as_ref()
      .ok_or("native export getter requires a synthetic Module")?;
    if state.status != STATUS_EVALUATED {
      return Err("native export getter requires an evaluated Module");
    }
    let value = properties(synthetic.namespace)
      .and_then(|properties| {
        properties.iter().find(|property| {
          same_property_key(property.key.cast(), (*key).cast())
        })
      })
      .ok_or("native export getter lost its declared export")?
      .value;
    Ok(value.cast::<Value>())
  })();
  match result {
    Ok(value) => {
      if let Some(info) = unsafe { callback_info(info) } {
        *info.return_slot = value;
      }
    }
    Err(error) => report(error.into()),
  }
}
