//! Minimal bindings to the Dart native API (`dart_native_api.h`):
//! `Dart_CObject` and posting messages to a Dart `ReceivePort`.

use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

pub type DartPort = i64;
pub type PostCObjectFn = unsafe extern "C" fn(DartPort, *mut DartCObject) -> bool;
pub type HandleFinalizer =
    unsafe extern "C" fn(isolate_callback_data: *mut c_void, peer: *mut c_void);

pub const K_NULL: i32 = 0;
pub const K_BOOL: i32 = 1;
pub const K_INT64: i32 = 3;
pub const K_DOUBLE: i32 = 4;
pub const K_STRING: i32 = 5;
pub const K_ARRAY: i32 = 6;
pub const K_EXTERNAL_TYPED_DATA: i32 = 8;
pub const K_UNMODIFIABLE_EXTERNAL_TYPED_DATA: i32 = 13;

pub const TYPED_DATA_UINT8: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DartArray {
    pub length: isize,
    pub values: *mut *mut DartCObject,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DartExternalTypedData {
    pub ty: i32,
    pub length: isize,
    pub data: *mut u8,
    pub peer: *mut c_void,
    pub callback: Option<HandleFinalizer>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union DartCObjectValue {
    pub as_bool: bool,
    pub as_int64: i64,
    pub as_double: f64,
    pub as_string: *const c_char,
    pub as_array: DartArray,
    pub as_external_typed_data: DartExternalTypedData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DartCObject {
    pub ty: i32,
    pub value: DartCObjectValue,
}

impl DartCObject {
    pub const fn null() -> Self {
        Self {
            ty: K_NULL,
            value: DartCObjectValue { as_int64: 0 },
        }
    }
    pub const fn bool(v: bool) -> Self {
        Self {
            ty: K_BOOL,
            value: DartCObjectValue { as_bool: v },
        }
    }
    pub const fn int(v: i64) -> Self {
        Self {
            ty: K_INT64,
            value: DartCObjectValue { as_int64: v },
        }
    }
    pub const fn double(v: f64) -> Self {
        Self {
            ty: K_DOUBLE,
            value: DartCObjectValue { as_double: v },
        }
    }
    /// `s` must be NUL-terminated UTF-8 that outlives the post call.
    pub const fn string(s: *const c_char) -> Self {
        Self {
            ty: K_STRING,
            value: DartCObjectValue { as_string: s },
        }
    }
    /// `values` must outlive the post call.
    pub fn array(values: &mut [*mut DartCObject]) -> Self {
        Self {
            ty: K_ARRAY,
            value: DartCObjectValue {
                as_array: DartArray {
                    length: values.len() as isize,
                    values: values.as_mut_ptr(),
                },
            },
        }
    }
}

/// Bytes handed over to Dart without copying. Dart owns the buffer after a
/// successful post and calls the finalizer when the typed data is collected.
pub struct ExternalBytes {
    pub object: DartCObject,
    drop_peer: unsafe fn(*mut c_void),
}

impl ExternalBytes {
    /// Hands over a value that owns the bytes at `data..data+len`.
    pub fn new<T: Send + 'static>(owner: T, data: *const u8, len: usize, modifiable: bool) -> Self {
        unsafe extern "C" fn finalize<T>(_: *mut c_void, peer: *mut c_void) {
            drop(unsafe { Box::from_raw(peer as *mut T) });
        }
        unsafe fn drop_peer<T>(peer: *mut c_void) {
            drop(unsafe { Box::from_raw(peer as *mut T) });
        }
        let peer = Box::into_raw(Box::new(owner)) as *mut c_void;
        Self {
            object: DartCObject {
                ty: if modifiable {
                    K_EXTERNAL_TYPED_DATA
                } else {
                    K_UNMODIFIABLE_EXTERNAL_TYPED_DATA
                },
                value: DartCObjectValue {
                    as_external_typed_data: DartExternalTypedData {
                        ty: TYPED_DATA_UINT8,
                        length: len as isize,
                        data: data as *mut u8,
                        peer,
                        callback: Some(finalize::<T>),
                    },
                },
            },
            drop_peer: drop_peer::<T>,
        }
    }

    /// Frees the buffer when it was not handed over (post failed).
    pub fn release(self) {
        unsafe { (self.drop_peer)(self.object.value.as_external_typed_data.peer) }
    }
}

static POST: OnceLock<PostCObjectFn> = OnceLock::new();

pub fn set_post(post: PostCObjectFn) {
    let _ = POST.set(post);
}

/// Posts `object` to `port`. Returns false if the port is closed.
pub fn post(port: DartPort, object: &mut DartCObject) -> bool {
    match POST.get() {
        Some(post) => unsafe { post(port, object) },
        None => false,
    }
}
