use std::path::Path;
use std::ptr;

use edge::{ABI_VERSION, function_by_name};

mod catalog;
pub use catalog::compile_edge_source_file;

mod vmbc;
mod wire;

const STATUS_OK: i32 = 0;
const STATUS_COMPILE_ERROR: i32 = 1;
const STATUS_INVALID_ARGUMENT: i32 = 2;
const STATUS_PANIC: i32 = 3;

const FROZEN_EDGE_ABI: u16 = 25;

#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn pdvm_compile_file_utf8(
    path_ptr: *const u8,
    path_len: usize,
    output_ptr: *mut *mut u8,
    output_len: *mut usize,
) -> i32 {
    if output_ptr.is_null() || output_len.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    unsafe {
        *output_ptr = ptr::null_mut();
        *output_len = 0;
    }
    if path_ptr.is_null() || path_len == 0 {
        write_output(output_ptr, output_len, b"source path is empty".to_vec());
        return STATUS_INVALID_ARGUMENT;
    }

    let result = std::panic::catch_unwind(|| {
        let path_bytes = unsafe { std::slice::from_raw_parts(path_ptr, path_len) };
        let path_text = std::str::from_utf8(path_bytes)
            .map_err(|error| (STATUS_INVALID_ARGUMENT, error.to_string()))?;
        compile_source_path(Path::new(path_text))
    });

    match result {
        Ok(Ok(vmbc)) => {
            write_output(output_ptr, output_len, vmbc);
            STATUS_OK
        }
        Ok(Err((status, message))) => {
            write_output(output_ptr, output_len, message.into_bytes());
            status
        }
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|value| (*value).to_owned())
                })
                .unwrap_or_else(|| "pd-vm compiler panicked".to_owned());
            write_output(output_ptr, output_len, message.into_bytes());
            STATUS_PANIC
        }
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn pdvm_free_buffer(buffer_ptr: *mut u8, buffer_len: usize) {
    if buffer_ptr.is_null() {
        return;
    }
    unsafe {
        let slice = ptr::slice_from_raw_parts_mut(buffer_ptr, buffer_len);
        drop(Box::from_raw(slice));
    }
}

fn compile_source_path(path: &Path) -> Result<Vec<u8>, (i32, String)> {
    if ABI_VERSION != FROZEN_EDGE_ABI {
        return Err((
            STATUS_COMPILE_ERROR,
            format!("stale pd-edge ABI {ABI_VERSION}, expected {FROZEN_EDGE_ABI}"),
        ));
    }

    let compiled = if source_needs_edge_catalog(path) {
        compile_edge_source_file(path).map_err(|error| (STATUS_COMPILE_ERROR, error.to_string()))?
    } else {
        vm::compile_source_file(path).map_err(|error| (STATUS_COMPILE_ERROR, error.to_string()))?
    };
    fail_closed_on_stale_catalog(&compiled.program)?;
    let local_count = compiled.locals;
    let program = compiled.program.with_local_count(local_count);
    vmbc::encode_program(program).map_err(|error| (STATUS_COMPILE_ERROR, error))
}

fn source_needs_edge_catalog(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|source| {
        source.contains("http::")
            || source.contains("proxy::")
            || source.contains("mqtt::")
            || source.contains("use http")
            || source.contains("use proxy")
            || source.contains("use mqtt")
    })
}

fn fail_closed_on_stale_catalog(program: &vm::Program) -> Result<(), (i32, String)> {
    let schemas = program.host_import_schemas();
    let schema_count = schemas.len();
    if schema_count != 0 && schema_count != program.imports.len() {
        return Err((
            STATUS_COMPILE_ERROR,
            format!(
                "host import schema count {schema_count} does not match import count {}",
                program.imports.len()
            ),
        ));
    }

    for (index, import) in program.imports.iter().enumerate() {
        let Some(abi_function) = function_by_name(&import.name) else {
            continue;
        };
        if schema_count == 0 {
            return Err((
                STATUS_COMPILE_ERROR,
                format!(
                    "catalog import '{}' is missing a typed host schema/fingerprint",
                    import.name
                ),
            ));
        }
        let Some(schema) = schemas[index].as_ref() else {
            return Err((
                STATUS_COMPILE_ERROR,
                format!(
                    "catalog import '{}' is missing a typed host schema/fingerprint",
                    import.name
                ),
            ));
        };
        if schema.name != import.name || schema.arity() != import.arity as usize {
            return Err((
                STATUS_COMPILE_ERROR,
                format!(
                    "catalog import '{}' schema does not match the published ABI contract",
                    import.name
                ),
            ));
        }
        if abi_function.name != schema.name || abi_function.arity as usize != schema.arity() {
            return Err((
                STATUS_COMPILE_ERROR,
                format!(
                    "catalog import '{}' drifted from pd-edge ABI {FROZEN_EDGE_ABI}",
                    import.name
                ),
            ));
        }
    }
    Ok(())
}

fn write_output(output_ptr: *mut *mut u8, output_len: *mut usize, bytes: Vec<u8>) {
    let mut bytes = bytes.into_boxed_slice();
    let pointer = bytes.as_mut_ptr();
    let length = bytes.len();
    std::mem::forget(bytes);
    unsafe {
        *output_ptr = pointer;
        *output_len = length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_path() {
        let mut output = ptr::null_mut();
        let mut length = 0;
        let status = pdvm_compile_file_utf8(ptr::null(), 0, &mut output, &mut length);
        assert_eq!(status, STATUS_INVALID_ARGUMENT);
        assert!(!output.is_null());
        pdvm_free_buffer(output, length);
    }

    #[test]
    fn frozen_edge_abi_is_version_25() {
        assert_eq!(ABI_VERSION, FROZEN_EDGE_ABI);
        assert!(
            edge::abi_json().contains("\"abi_version\": 25"),
            "published edge ABI JSON must record version 25"
        );
    }

    #[test]
    fn pinned_rust_runtime_matches_shared_callable_parity_fixture() {
        let source = include_str!("../../../tests/fixtures/callable-parity.rss");
        let compiled = vm::compile_source(source).expect("shared callable fixture should compile");
        let program = compiled.program.with_local_count(compiled.locals);
        assert_eq!(
            vmbc::encode_program(program.clone()).expect("compiler-only encoding"),
            vm::encode_program(&program).expect("frozen upstream encoding")
        );
        let mut runtime = vm::Vm::new(program);

        let status = runtime
            .run()
            .expect("shared callable fixture should execute");

        assert_eq!(status, vm::VmStatus::Halted);
        assert_eq!(
            runtime.stack(),
            &[vm::Value::Int(42), vm::Value::Int(15), vm::Value::Int(1)]
        );
    }
}
