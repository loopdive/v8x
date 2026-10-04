//! AOT export dispatch stays in the graph that compiled the callable.
use super::*;
use wasmtime::{AsContextMut, RootScope, Val};

/// Native Error identity comes from the compiler's guarded carrier ABI, never
/// from public properties such as name/message that an ordinary object can copy.
pub(super) fn native_error_snapshot(
  mut context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handle: f64,
) -> Result<Option<(f64, f64)>, String> {
  let mut graphs = context.as_context().data().aot_call_graphs.clone();
  graphs.push(realm);
  let mut scope = RootScope::new(&mut context);
  let unwrap = realm
    .get_func(&mut scope, "__v8x_value_unwrap")
    .ok_or("realm lacks same-store value unwrap ABI")?;
  let mut value = [Val::ExternRef(None)];
  unwrap
    .call(&mut scope, &[Val::F64(handle.to_bits())], &mut value)
    .map_err(|error| format!("unwrap native Error: {error:#}"))?;
  for graph in graphs {
    let Some(classify) =
      graph.get_func(&mut scope, "__error_boundary_is_native")
    else {
      continue;
    };
    let mut result = [Val::I32(0)];
    classify
      .call(&mut scope, &value, &mut result)
      .map_err(|error| format!("classify native Error: {error:#}"))?;
    match result[0].i32() {
      Some(1) => {
        let keep = realm
          .get_func(&mut scope, "__v8x_value_keep")
          .ok_or("realm lacks same-store value keep ABI")?;
        let mut fields = [0.0; 2];
        for (index, name) in
          ["__error_boundary_name", "__error_boundary_message"]
            .iter()
            .enumerate()
        {
          let read = graph
            .get_func(&mut scope, name)
            .ok_or("native Error classifier lacks field ABI")?;
          let mut field = [Val::ExternRef(None)];
          read
            .call(&mut scope, &value, &mut field)
            .map_err(|error| format!("read native Error field: {error:#}"))?;
          let mut handle = [Val::F64(0)];
          keep
            .call(&mut scope, &field, &mut handle)
            .map_err(|error| format!("retain native Error field: {error:#}"))?;
          fields[index] =
            handle[0].f64().ok_or("invalid Error field handle")?;
        }
        return Ok(Some((fields[0], fields[1])));
      }
      Some(0) => {}
      _ => {
        return Err("native Error classifier must return zero or one".into());
      }
    }
  }
  Ok(None)
}

/// Drain every graph in this store to a verified fixed point. Graphs with
/// promise jobs must publish both drain and pending-count exports.
pub(super) fn drain_microtasks(
  mut context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
) -> Result<bool, String> {
  let mut graphs = context.as_context().data().aot_call_graphs.clone();
  graphs.push(realm);
  let mut did_work = false;
  loop {
    let mut pass_work = false;
    for graph in &graphs {
      let Some(drain) = graph.get_func(&mut context, "__drain_microtasks")
      else {
        continue;
      };
      let pending = graph
        .get_func(&mut context, "__microtasks_pending")
        .ok_or("compiled microtask queue lacks pending-count ABI")?
        .typed::<(), i32>(&context)
        .map_err(|error| format!("type compiled microtask count: {error}"))?;
      let count = pending
        .call(&mut context, ())
        .map_err(|error| format!("read compiled microtask count: {error:#}"))?;
      if count < 0 {
        return Err(format!("invalid compiled microtask count {count}"));
      }
      if count == 0 {
        continue;
      }
      pass_work = true;
      did_work = true;
      drain
        .typed::<(), ()>(&context)
        .map_err(|error| format!("type compiled microtask drain: {error}"))?
        .call(&mut context, ())
        .map_err(|error| format!("drain compiled microtasks: {error:#}"))?;
    }
    if !pass_work {
      return Ok(did_work);
    }
  }
}

/// Read the guarded native Promise ABI, retaining the live GC result in the
/// realm's same-store handle table. -1 means this module cannot classify it.
pub(super) fn promise_snapshot(
  mut context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handle: f64,
) -> Result<Option<(i32, f64)>, String> {
  let mut graphs = context.as_context().data().aot_call_graphs.clone();
  graphs.push(realm);
  let mut scope = RootScope::new(&mut context);
  let unwrap = realm
    .get_func(&mut scope, "__v8x_value_unwrap")
    .ok_or("realm lacks same-store value unwrap ABI")?;
  let keep = realm
    .get_func(&mut scope, "__v8x_value_keep")
    .ok_or("realm lacks same-store value keep ABI")?;
  let mut promise = [Val::ExternRef(None)];
  unwrap
    .call(&mut scope, &[Val::F64(handle.to_bits())], &mut promise)
    .map_err(|error| format!("unwrap promise: {error:#}"))?;
  for graph in graphs {
    let Some(read_state) =
      graph.get_func(&mut scope, "__promise_boundary_state")
    else {
      continue;
    };
    let mut state = [Val::I32(-1)];
    read_state
      .call(&mut scope, &promise, &mut state)
      .map_err(|error| format!("read compiled promise state: {error:#}"))?;
    let state = state[0].i32().ok_or("promise state ABI is not i32")?;
    if state == -1 {
      continue;
    }
    if !(0..=2).contains(&state) {
      return Err(format!("invalid compiled promise state {state}"));
    }
    let read_value = graph
      .get_func(&mut scope, "__promise_boundary_value")
      .ok_or("compiled promise lacks value ABI")?;
    let mut value = [Val::ExternRef(None)];
    read_value
      .call(&mut scope, &promise, &mut value)
      .map_err(|error| format!("read compiled promise value: {error:#}"))?;
    let mut result = [Val::F64(0)];
    keep
      .call(&mut scope, &value, &mut result)
      .map_err(|error| format!("retain compiled promise value: {error:#}"))?;
    return Ok(Some((
      state,
      result[0].f64().ok_or("invalid promise result handle")?,
    )));
  }
  Ok(None)
}

/// Read or mark the handler bit in the graph that owns this Promise.
pub(super) fn promise_handler(
  mut context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handle: f64,
  mark: bool,
) -> Result<Option<bool>, String> {
  let mut graphs = context.as_context().data().aot_call_graphs.clone();
  graphs.push(realm);
  let mut scope = RootScope::new(&mut context);
  let unwrap = realm
    .get_func(&mut scope, "__v8x_value_unwrap")
    .ok_or("realm lacks same-store value unwrap ABI")?;
  let mut promise = [Val::ExternRef(None)];
  unwrap
    .call(&mut scope, &[Val::F64(handle.to_bits())], &mut promise)
    .map_err(|error| format!("unwrap promise handler receiver: {error:#}"))?;
  for graph in graphs {
    let Some(state) = graph.get_func(&mut scope, "__promise_boundary_state")
    else {
      continue;
    };
    let mut classification = [Val::I32(-1)];
    state
      .call(&mut scope, &promise, &mut classification)
      .map_err(|error| {
        format!("classify promise handler receiver: {error:#}")
      })?;
    let classification = classification[0]
      .i32()
      .ok_or("promise state ABI is not i32")?;
    if classification == -1 {
      continue;
    }
    if !(0..=2).contains(&classification) {
      return Err("invalid compiled Promise state".to_string());
    }
    let name = if mark {
      "__promise_boundary_mark_handled"
    } else {
      "__promise_boundary_has_handler"
    };
    let access = graph
      .get_func(&mut scope, name)
      .ok_or("compiled Promise lacks handler ABI; rebuild artifact")?;
    let mut handled = [Val::I32(-1)];
    access
      .call(&mut scope, &promise, &mut handled)
      .map_err(|error| {
        format!("access compiled Promise handler state: {error:#}")
      })?;
    return match handled[0].i32() {
      Some(0) if !mark => Ok(Some(false)),
      Some(1) => Ok(Some(true)),
      _ => Err("invalid compiled Promise handler state".to_string()),
    };
  }
  Ok(None)
}

pub(super) fn call(
  mut context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: [f64; 3],
) -> Result<Option<(bool, f64)>, String> {
  if let Some(completion) = dispatch(
    &mut context,
    realm,
    &handles,
    "__v8x_graph_can_call_export",
    "__v8x_graph_call_export",
  )? {
    return Ok(Some(completion));
  }
  // Bootstrap callables live in the realm itself, not a registered application
  // graph. Retain their thrown JS value just like graph dispatch does. Rendering
  // the Wasmtime error into a new Error loses both object identity and message.
  let mut scope = RootScope::new(&mut context);
  let Some(call) = realm.get_func(&mut scope, "__v8x_value_call") else {
    return Ok(None);
  };
  let args = handles.map(|handle| Val::F64(handle.to_bits()));
  let mut result = [Val::F64(0)];
  let success = match call.call(&mut scope, &args, &mut result) {
    Ok(()) => true,
    Err(error) => {
      let exception = scope
        .as_context_mut()
        .take_pending_exception()
        .ok_or_else(|| format!("realm callable trapped: {error:#}"))?;
      let fields = exception
        .fields(&mut scope)
        .map_err(|error| format!("read realm exception: {error:#}"))?
        .collect::<Vec<_>>();
      let [payload @ Val::ExternRef(_)] = fields.as_slice() else {
        return Err("realm exception must carry one JS value".into());
      };
      let keep = realm
        .get_func(&mut scope, "__v8x_value_keep")
        .ok_or("realm lacks exception value keep ABI")?;
      keep
        .call(&mut scope, &[*payload], &mut result)
        .map_err(|error| format!("retain realm exception: {error:#}"))?;
      false
    }
  };
  Ok(Some((
    success,
    result[0].f64().ok_or("invalid realm completion handle")?,
  )))
}

pub(super) fn promise_then(
  context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: [f64; 3],
) -> Result<Option<(bool, f64)>, String> {
  dispatch(
    context,
    realm,
    &handles,
    "__v8x_graph_can_access_export",
    "__v8x_graph_promise_then_export",
  )
}

pub(super) fn get(
  context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: [f64; 2],
) -> Result<Option<(bool, f64)>, String> {
  dispatch(
    context,
    realm,
    &handles,
    "__v8x_graph_can_access_export",
    "__v8x_graph_get_export",
  )
}

pub(super) fn set(
  context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: [f64; 3],
) -> Result<Option<(bool, f64)>, String> {
  dispatch(
    context,
    realm,
    &handles,
    "__v8x_graph_can_access_export",
    "__v8x_graph_set_export",
  )
}

pub(super) fn get_prototype(
  context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: [f64; 1],
) -> Result<Option<(bool, f64)>, String> {
  dispatch(
    context,
    realm,
    &handles,
    "__v8x_graph_can_access_export",
    "__v8x_graph_get_prototype_export",
  )
}

pub(super) fn reflection(
  context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: &[f64],
  operation: &str,
) -> Result<Option<(bool, f64)>, String> {
  let export = match operation {
    "names" => "__v8x_graph_own_names_export",
    "symbols" => "__v8x_graph_own_symbols_export",
    "descriptor" => "__v8x_graph_descriptor_export",
    _ => return Err("unknown native reflection operation".into()),
  };
  dispatch(
    context,
    realm,
    handles,
    "__v8x_graph_can_access_export",
    export,
  )
}

pub(super) fn set_prototype(
  context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: [f64; 2],
) -> Result<Option<(bool, f64)>, String> {
  dispatch(
    context,
    realm,
    &handles,
    "__v8x_graph_can_access_export",
    "__v8x_graph_set_prototype_export",
  )
}

fn dispatch(
  mut context: impl AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: &[f64],
  matcher_name: &str,
  dispatch_name: &str,
) -> Result<Option<(bool, f64)>, String> {
  let graphs = context.as_context().data().aot_call_graphs.clone();
  if graphs.is_empty() {
    return Ok(None);
  }
  let mut scope = RootScope::new(&mut context);
  let routes = graphs
    .iter()
    .rev()
    .flat_map(|graph| {
      let mut routes = Vec::new();
      if dispatch_name == "__v8x_graph_get_export"
        && let Some(owns) = graph.get_func(&mut scope, "__v8x_graph_owns")
      {
        routes.push((*graph, owns, "__v8x_graph_get_owned_export", true));
      }
      if let Some(matcher) = graph.get_func(&mut scope, matcher_name) {
        routes.insert(0, (*graph, matcher, dispatch_name, false));
      } else {
        let script_export = match dispatch_name {
          "__v8x_graph_get_export" => "__v8x_script_get_export",
          "__v8x_graph_call_export" => "__v8x_script_call_export",
          "__v8x_graph_own_names_export" => "__v8x_script_own_names_export",
          "__v8x_graph_own_symbols_export" => "__v8x_script_own_symbols_export",
          "__v8x_graph_descriptor_export" => "__v8x_script_descriptor_export",
          _ => return routes,
        };
        // Missing reflection on a matching Script is an error below, not an
        // empty wrapper enumeration or fallback through a foreign Context.
        if !matches!(
          dispatch_name,
          "__v8x_graph_own_names_export"
            | "__v8x_graph_own_symbols_export"
            | "__v8x_graph_descriptor_export"
        ) && graph.get_func(&mut scope, script_export).is_none()
        {
          return routes;
        }
        if let Some(matcher) = graph.get_func(&mut scope, "localOwns") {
          routes.push((*graph, matcher, script_export, true));
        }
      }
      routes
    })
    .collect::<Vec<_>>();
  if routes.is_empty() {
    return Ok(None);
  }
  let unwrap = realm
    .get_func(&mut scope, "__v8x_value_unwrap")
    .ok_or("realm lacks same-store value unwrap ABI")?;
  let keep = realm
    .get_func(&mut scope, "__v8x_value_keep")
    .ok_or("realm lacks same-store value keep ABI")?;
  let mut callable = [Val::ExternRef(None)];
  unwrap
    .call(&mut scope, &[Val::F64(handles[0].to_bits())], &mut callable)
    .map_err(|error| format!("unwrap graph callable: {error:#}"))?;
  for (graph, matcher, dispatch_name, script) in routes {
    // Export reachability is not allocation ownership: a newer graph may
    // re-export an older graph's function. Its structurally compatible call
    // dispatcher cannot install `this` in that function's original globals.
    if !script && dispatch_name == "__v8x_graph_call_export" {
      let owns = graph
        .get_func(&mut scope, "__v8x_graph_owns")
        .ok_or("callable graph lacks allocation ownership; rebuild artifact")?;
      let mut owned = [Val::I32(0)];
      owns
        .call(&mut scope, &callable, &mut owned)
        .map_err(|error| {
          format!("classify callable allocation owner: {error:#}")
        })?;
      match owned[0].i32() {
        Some(0) => continue,
        Some(1) => {}
        _ => {
          return Err("callable ownership ABI must return zero or one".into());
        }
      }
    }
    let mut matches = [if script { Val::I32(0) } else { Val::F64(0) }];
    matcher
      .call(&mut scope, &callable, &mut matches)
      .map_err(|error| format!("classify graph callable: {error:#}"))?;
    if if script {
      matches[0].i32() != Some(1)
    } else {
      matches[0].f64() != Some(1.0)
    } {
      continue;
    }
    let dispatch = graph
      .get_func(&mut scope, dispatch_name)
      .ok_or("matching graph lacks call export")?;
    let mut args = vec![callable[0]];
    for handle in &handles[1..] {
      let mut value = [Val::ExternRef(None)];
      unwrap
        .call(&mut scope, &[Val::F64(handle.to_bits())], &mut value)
        .map_err(|error| format!("unwrap graph call argument: {error:#}"))?;
      args.push(value[0]);
    }
    let mut value = [Val::ExternRef(None)];
    let success = match dispatch.call(&mut scope, &args, &mut value) {
      Ok(()) => true,
      Err(error) => {
        let exception = scope
          .as_context_mut()
          .take_pending_exception()
          .ok_or_else(|| format!("AOT graph call trapped: {error:#}"))?;
        let fields = exception
          .fields(&mut scope)
          .map_err(|error| format!("read graph exception: {error:#}"))?
          .collect::<Vec<_>>();
        let [payload] = fields.as_slice() else {
          return Err("graph exception must carry one value".to_string());
        };
        value[0] = *payload;
        false
      }
    };
    let mut result = [Val::F64(0)];
    keep
      .call(&mut scope, &value, &mut result)
      .map_err(|error| format!("retain graph completion: {error:#}"))?;
    return Ok(Some((
      success,
      result[0].f64().ok_or("invalid completion handle")?,
    )));
  }
  Ok(None)
}
