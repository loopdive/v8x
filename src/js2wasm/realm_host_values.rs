use super::*;
use std::collections::HashMap;

struct Node {
  host: *const Object,
  array_len: Option<usize>,
  callback: bool,
  prototype: Option<*const Value>,
  properties: Vec<(String, *const Value, u32)>,
}

// Inspect the whole graph before allocating or publishing bindings. In
// particular an unsupported nested host value cannot partially adopt its
// parent. No isolate heap reference is held across a Wasmtime call.
fn snapshot(
  owner: &Rc<RefCell<DenoRuntime>>,
  root: *const Value,
) -> Result<Vec<Node>, String> {
  let mut pending = vec![root];
  let mut visited = HashSet::new();
  let mut nodes = Vec::new();
  while let Some(value) = pending.pop() {
    if !visited.insert(value as usize) {
      continue;
    }
    if let Some(entry) = binding(value.cast()) {
      if !Rc::ptr_eq(owner, &entry.runtime) {
        return Err("cannot transfer an object between realms".to_string());
      }
      continue;
    }
    let mut callback = false;
    let mut prototype = None;
    let (array_len, properties) = match unsafe { heap_value(value) } {
      Some(
        HeapValue::Undefined
        | HeapValue::Null
        | HeapValue::Boolean(_)
        | HeapValue::Number(_)
        | HeapValue::String(_)
        | HeapValue::Symbol(_),
      ) => continue,
      Some(HeapValue::Object(state)) => {
        prototype = state.prototype;
        if let Some(value) = prototype {
          if !is_valid_prototype(value) {
            return Err("invalid host prototype".into());
          }
          pending.push(value);
        }
        // Internal fields remain private to this exact Rust wrapper. Adoption
        // publishes its identity without copying native pointers into Wasm.
        (None, state.properties.clone())
      }
      Some(HeapValue::Function(state)) => {
        callback = true;
        (None, state.properties.clone())
      }
      Some(HeapValue::Array(state)) => {
        let mut properties = Vec::new();
        for (index, value) in state.elements.iter().enumerate() {
          properties.push((index.to_string(), *value, 0));
          pending.push(*value);
        }
        nodes.push(Node {
          host: value.cast(),
          array_len: Some(state.elements.len()),
          callback: false,
          prototype: None,
          properties,
        });
        (Some(state.elements.len()), state.properties.clone())
      }
      _ => return Err("unsupported exotic host graph value".to_string()),
    };
    let mut entries = Vec::new();
    for property in properties {
      if property.attributes & !7 != 0 {
        return Err("unsupported host property attributes".to_string());
      }
      let key = unsafe { string_value(property.key) }
        .ok_or_else(|| "symbol host keys are not transferable yet".to_string())?
        .to_string();
      let value = property.value.cast();
      pending.push(value);
      entries.push((key, value, property.attributes));
    }
    if array_len.is_some() {
      nodes.last_mut().unwrap().properties.extend(entries);
    } else {
      nodes.push(Node {
        host: value.cast(),
        array_len,
        callback,
        prototype,
        properties: entries,
      });
    }
  }
  Ok(nodes)
}

pub(super) fn transfer(
  runtime: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  root: *const Value,
) -> Result<RealmValue, String> {
  transfer_into(runtime, owner, root, None)
}

// A seeded root uses an existing realm object rather than creating a detached
// copy. Call only before consumers capture properties that will be replaced.
pub(super) fn transfer_into(
  runtime: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  root: *const Value,
  target: Option<RealmValue>,
) -> Result<RealmValue, String> {
  if let Some(value) = target {
    runtime.realm_check(value)?;
  }
  let nodes = snapshot(owner, root)?;
  let mut handles = HashMap::new();
  for node in &nodes {
    let value = if node.host == root.cast() && target.is_some() {
      target.unwrap()
    } else if node.callback {
      host_callbacks::allocate(runtime, owner, node.host.cast())?
    } else if node.array_len.is_some() {
      runtime.realm_array()?
    } else {
      runtime.realm_object()?
    };
    handles.insert(node.host as usize, value);
  }
  for node in &nodes {
    let object = handles[&(node.host as usize)];
    if let Some(prototype) = node.prototype {
      let prototype = match handles.get(&(prototype as usize)) {
        Some(value) => *value,
        None => into_realm(runtime, owner, prototype)?,
      };
      if !runtime.realm_set_prototype(object, prototype)? {
        return Err("compiled realm rejected host prototype".into());
      }
    }
    if let Some(length) = node.array_len {
      let key =
        runtime.realm_string(&"length".encode_utf16().collect::<Vec<_>>())?;
      let value = runtime.realm_number(length as f64)?;
      runtime.realm_set(object, key, value)?;
    }
    let mut definitions = Vec::new();
    for (key, value, attributes) in &node.properties {
      let key =
        runtime.realm_string(&key.encode_utf16().collect::<Vec<_>>())?;
      let value = match handles.get(&(*value as usize)) {
        Some(value) => *value,
        None => into_realm(runtime, owner, *value)?,
      };
      if target.is_none() {
        definitions.push((key, value, *attributes));
      } else {
        runtime.realm_define_data(object, key, value, *attributes)?;
      }
    }
    // Only newly allocated, unpublished nodes. Seeded or already visible
    // objects retain immediate operations and exception ordering.
    if !definitions.is_empty() {
      runtime.realm_define_many(object, &definitions)?;
    }
  }
  let result = handles[&(root as usize)];
  // Publish only after every property is initialized. Existing Rust objects
  // now keep their identity but route subsequent access to the compiled realm.
  let bindings = nodes.into_iter().map(|node| RealmObjectBinding {
    host: node.host,
    runtime: owner.clone(),
    value: handles[&(node.host as usize)],
  });
  unsafe { isolate_state(current_isolate()) }
    .realm_objects
    .extend(bindings);
  Ok(result)
}
