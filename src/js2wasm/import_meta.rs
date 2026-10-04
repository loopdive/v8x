use super::*;

/// The loader owns import-meta properties. Cache by native Module identity,
/// never by URL: different modules may have the same resource name.
pub(super) fn get(
  module: *const Module,
  context: *const Context,
) -> Result<*const Object, String> {
  let isolate = current_isolate();
  if isolate.is_null() {
    return Err("import.meta has no current isolate".to_string());
  }
  // Validate ownership before dereferencing handles or invoking embedder code.
  let owns = |pointer: *const HeapValue| {
    unsafe { isolate_state(isolate) }
      .values
      .iter()
      .any(|value| std::ptr::addr_eq(*value, pointer))
  };
  if !owns(module.cast()) || !owns(context.cast()) {
    return Err("import.meta handles belong to another isolate".to_string());
  }
  if !matches!(unsafe { heap_value(context) }, Some(HeapValue::Context(_))) {
    return Err("import.meta requires a live Context".to_string());
  }
  let cached = unsafe { module_state(module) }
    .ok_or_else(|| "import.meta requires a Module".to_string())?
    .import_meta;
  if let Some((owner, meta)) = cached {
    return if owner == context {
      Ok(meta)
    } else {
      Err("import.meta module belongs to another Context".to_string())
    };
  }
  let meta = new_object(isolate);
  // import.meta is a null-prototype object. Preserve that explicit edge when
  // the native object is adopted into the compiled realm.
  let null = v8__Null(isolate).cast();
  if let Some(HeapValue::Object(state)) = unsafe { heap_value_mut(meta) } {
    state.prototype = Some(null);
  }
  // Publish before calling the embedder. A reentrant access must see the same
  // object, and no mutable Module/Isolate borrow may span the callback.
  unsafe { module_state(module) }.unwrap().import_meta = Some((context, meta));
  let callback = unsafe { isolate_state(isolate) }.import_meta_callback;
  if let Some(callback) = callback {
    unsafe {
      callback(
        crate::Local::from_raw(context).unwrap(),
        crate::Local::from_raw(module).unwrap(),
        crate::Local::from_raw(meta).unwrap(),
      );
    }
  }
  Ok(meta)
}
