use super::*;

/// Keeps an ordinary host ArrayBuffer's allocation alive while a realm uses it.
/// Access stays on the isolate thread; this is not SharedArrayBuffer support.
pub(crate) struct RetainedHostBuffer(SharedRepr);

impl RetainedHostBuffer {
  pub(super) fn new(buffer: *const ArrayBuffer) -> Result<Self, String> {
    let state = unsafe { array_buffer_state(buffer) }
      .ok_or_else(|| "invalid host ArrayBuffer".to_string())?;
    let backing =
      unsafe { backing_store_state(state.backing_store.object.cast()) }
        .ok_or_else(|| "missing ArrayBuffer backing store".to_string())?;
    if backing.byte_length > i32::MAX as usize
      || (backing.byte_length != 0 && backing.data.is_null())
      || state.backing_store.references.is_null()
    {
      return Err("unsupported host ArrayBuffer backing store".to_string());
    }
    Ok(Self(retain_backing_store(state.backing_store)))
  }

  pub(crate) fn identity(&self) -> usize {
    self.0.object as usize
  }

  pub(crate) fn len(&self) -> usize {
    unsafe {
      backing_store_state(self.0.object.cast())
        .unwrap()
        .byte_length
    }
  }

  pub(crate) fn read(&self, index: usize) -> u8 {
    assert!(index < self.len());
    unsafe {
      backing_store_state(self.0.object.cast())
        .unwrap()
        .data
        .cast::<u8>()
        .add(index)
        .read()
    }
  }

  pub(crate) fn write(&mut self, index: usize, byte: u8) {
    assert!(index < self.len());
    unsafe {
      backing_store_state(self.0.object.cast())
        .unwrap()
        .data
        .cast::<u8>()
        .add(index)
        .write(byte)
    }
  }
}

impl Drop for RetainedHostBuffer {
  fn drop(&mut self) {
    release_backing_store(self.0);
  }
}
