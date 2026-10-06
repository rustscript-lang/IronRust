//! VMBC v13 encoder extracted from rustscript src/vmbc.rs at
//! b1d6cffede77f49410bf63525f30b9a46b02dc01 (see LICENSE.rustscript).
//! The pinned upstream gates its wire API behind `runtime`. Keep only the
//! encoding routines here, using public compiler metadata accessors, so the
//! distributed compiler does not link a Rust VM. Tests compare bytes against
//! the pinned upstream encoder, including the complete example corpus.

use vm::debug_info::DebugInfo;
use vm::host_api::{HostImportSchema, HostParamPassing, HostTypeSchema, MAX_HOST_SCHEMA_DEPTH};
use vm::{CallableKind, CallableTarget, Program, TypeMap, TypeSchema, Value};

const MAGIC: [u8; 4] = *b"VMBC";
const VERSION_V13: u16 = 13;
const FLAGS: u16 = 0;
const MAX_CONSTANT_DEPTH: usize = 64;

#[derive(Debug)]
pub enum WireError {
    UnsupportedConstantType(&'static str),
    LengthTooLarge(&'static str, usize),
    HostSchemaImportMismatch,
    InvalidHostSchemaComplexity(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedConstantType(kind) => {
                write!(f, "unsupported constant type for wire format: {kind}")
            }
            Self::LengthTooLarge(field, len) => write!(f, "{field} length too large: {len}"),
            Self::HostSchemaImportMismatch => {
                write!(f, "host import schema does not match its import")
            }
            Self::InvalidHostSchemaComplexity(reason) => {
                write!(f, "invalid host schema complexity: {reason}")
            }
        }
    }
}

impl std::error::Error for WireError {}

fn write_constant(value: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), WireError> {
    if depth >= MAX_CONSTANT_DEPTH {
        return Err(WireError::LengthTooLarge("constant nesting depth", depth));
    }
    match value {
        Value::Int(value) => {
            out.push(0);
            out.extend_from_slice(&value.to_le_bytes());
        }
        Value::Bool(value) => {
            out.push(1);
            out.push(u8::from(*value));
        }
        Value::String(value) => {
            out.push(2);
            write_u32_len("constant string", value.len(), out)?;
            out.extend_from_slice(value.as_bytes());
        }
        Value::Float(value) => {
            out.push(3);
            out.extend_from_slice(&value.to_le_bytes());
        }
        Value::Null => out.push(4),
        Value::Bytes(value) => {
            out.push(5);
            write_u32_len("constant bytes", value.len(), out)?;
            out.extend_from_slice(value.as_slice());
        }
        Value::Array(values) => {
            out.push(6);
            write_u32_count("constant array", values.len(), out)?;
            for value in values.iter() {
                write_constant(value, out, depth + 1)?;
            }
        }
        Value::Map(entries) => {
            out.push(7);
            write_u32_count("constant map", entries.len(), out)?;
            for (key, value) in entries.iter() {
                write_constant(key, out, depth + 1)?;
                write_constant(value, out, depth + 1)?;
            }
        }
        Value::Callable(_) => return Err(WireError::UnsupportedConstantType("callable")),
    }
    Ok(())
}

pub fn encode_program(program: &Program) -> Result<Vec<u8>, WireError> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION_V13.to_le_bytes());
    out.extend_from_slice(&FLAGS.to_le_bytes());
    write_u32_count("constants", program.constants.len(), &mut out)?;

    for constant in &program.constants {
        write_constant(constant, &mut out, 0)?;
    }

    write_u32_len("code", program.code.len(), &mut out)?;
    out.extend_from_slice(&program.code);

    write_u32_count("imports", program.imports.len(), &mut out)?;
    if !program.host_import_schemas().is_empty() {
        if program.host_import_schemas().len() != program.imports.len() {
            return Err(WireError::HostSchemaImportMismatch);
        }
        vm::validate_host_import_schemas(
            &program
                .host_import_schemas()
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
        )
        .map_err(|error| WireError::InvalidHostSchemaComplexity(error.to_string()))?;
    }
    for (index, import) in program.imports.iter().enumerate() {
        write_string("import name", &import.name, &mut out)?;
        out.push(import.arity);
        out.push(import.return_type as u8);
        let schema = if program.host_import_schemas().is_empty() {
            None
        } else {
            if program.host_import_schemas().len() != program.imports.len() {
                return Err(WireError::HostSchemaImportMismatch);
            }
            program.host_import_schemas()[index].as_ref()
        };
        write_optional_host_import_schema(schema, &mut out)?;
    }

    write_type_map(&mut out, program.type_map.as_ref())?;
    write_debug_info(&mut out, program.debug.as_ref())?;
    write_callable_metadata(&mut out, program)?;
    write_named_struct_decls(&mut out, program)?;

    Ok(out)
}

fn write_callable_metadata(out: &mut Vec<u8>, program: &Program) -> Result<(), WireError> {
    write_u32_count("script functions", program.script_functions.len(), out)?;
    for function in &program.script_functions {
        out.extend_from_slice(&function.entry_ip.to_le_bytes());
        out.extend_from_slice(&function.end_ip.to_le_bytes());
    }

    write_u32_count(
        "callable prototypes",
        program.callable_prototypes.len(),
        out,
    )?;
    for prototype in &program.callable_prototypes {
        out.push(match prototype.kind {
            CallableKind::FunctionItem => 0,
            CallableKind::Closure => 1,
            CallableKind::HostFunction => 2,
        });
        match prototype.target {
            CallableTarget::ScriptFunction(id) => {
                out.push(0);
                out.extend_from_slice(&id.to_le_bytes());
            }
            CallableTarget::HostImport(id) => {
                out.push(1);
                out.extend_from_slice(&u32::from(id).to_le_bytes());
            }
        }
        out.push(prototype.arity);
        write_u32_count("callable frame locals", prototype.frame_local_count, out)?;
        write_u16_list("callable parameters", &prototype.parameter_slots, out)?;
        write_u16_list(
            "callable capture sources",
            &prototype.capture_source_slots,
            out,
        )?;
        write_u16_list("callable captures", &prototype.capture_slots, out)?;
        write_u32_count("callable capture modes", prototype.capture_modes.len(), out)?;
        for mode in &prototype.capture_modes {
            out.push(*mode as u8);
        }
        match prototype.self_slot {
            Some(slot) => {
                out.push(1);
                out.extend_from_slice(&slot.to_le_bytes());
            }
            None => out.push(0),
        }
        match &prototype.schema {
            Some(schema) => {
                out.push(1);
                write_schema(schema, out)?;
            }
            None => out.push(0),
        }
    }

    write_u32_count("function regions", program.function_regions.len(), out)?;
    for region in &program.function_regions {
        out.extend_from_slice(&region.start_ip.to_le_bytes());
        out.extend_from_slice(&region.end_ip.to_le_bytes());
        match region.prototype_id {
            Some(id) => {
                out.push(1);
                out.extend_from_slice(&id.to_le_bytes());
            }
            None => out.push(0),
        }
    }

    write_u32_count(
        "root callable bindings",
        program.root_callable_bindings.len(),
        out,
    )?;
    for binding in &program.root_callable_bindings {
        out.extend_from_slice(&binding.local_slot.to_le_bytes());
        out.extend_from_slice(&binding.prototype_id.to_le_bytes());
    }
    write_u32_count("exported callables", program.exported_callables.len(), out)?;
    for exported in &program.exported_callables {
        write_string("exported callable name", &exported.name, out)?;
        out.extend_from_slice(&exported.local_slot.to_le_bytes());
    }
    Ok(())
}

fn write_named_struct_decls(out: &mut Vec<u8>, program: &Program) -> Result<(), WireError> {
    let mut decls = program.named_struct_decls().values().collect::<Vec<_>>();
    decls.sort_unstable_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
    write_u32_count("named struct decls", decls.len(), out)?;
    for decl in decls {
        write_string("named struct name", &decl.name, out)?;
        write_u32_count("named struct type params", decl.type_params.len(), out)?;
        for type_param in &decl.type_params {
            write_string("named struct type param", type_param, out)?;
        }
        write_schema(&decl.body_schema, out)?;
    }
    Ok(())
}

fn write_u16_list(field: &'static str, values: &[u16], out: &mut Vec<u8>) -> Result<(), WireError> {
    write_u32_count(field, values.len(), out)?;
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    Ok(())
}

fn write_debug_info(out: &mut Vec<u8>, debug: Option<&DebugInfo>) -> Result<(), WireError> {
    match debug {
        None => {
            out.push(0);
            Ok(())
        }
        Some(debug) => {
            out.push(1);

            match &debug.source {
                None => out.push(0),
                Some(source) => {
                    out.push(1);
                    write_string("debug source", source, out)?;
                }
            }

            write_u32_count("debug lines", debug.lines.len(), out)?;
            for line in &debug.lines {
                out.extend_from_slice(&line.offset.to_le_bytes());
                out.extend_from_slice(&line.line.to_le_bytes());
            }

            write_u32_count("debug functions", debug.functions.len(), out)?;
            for function in &debug.functions {
                write_string("debug function name", &function.name, out)?;
                write_u32_count("debug function args", function.args.len(), out)?;
                for arg in &function.args {
                    write_string("debug arg name", &arg.name, out)?;
                    out.push(arg.position);
                }
            }

            write_u32_count("debug locals", debug.locals.len(), out)?;
            for local in &debug.locals {
                write_string("debug local name", &local.name, out)?;
                out.push(local.index);
                write_optional_u32(local.declared_line, out);
                write_optional_u32(local.last_line, out);
            }

            Ok(())
        }
    }
}

fn write_type_map(out: &mut Vec<u8>, type_map: Option<&TypeMap>) -> Result<(), WireError> {
    let Some(type_map) = type_map else {
        out.push(0);
        return Ok(());
    };

    out.push(1);
    out.push(u8::from(type_map.strict_types));
    write_u32_count("type map locals", type_map.local_types.len(), out)?;
    for ty in &type_map.local_types {
        out.push(*ty as u8);
    }
    for schema in &type_map.local_schemas {
        write_optional_schema(schema.as_ref(), out)?;
    }
    write_bool_slice("type map callable slots", &type_map.callable_slots, out)?;
    write_bool_slice("type map optional slots", &type_map.optional_slots, out)?;

    write_u32_count("type map operands", type_map.operand_types.len(), out)?;
    let mut operand_entries = type_map
        .operand_types
        .iter()
        .map(|(offset, pair)| (*offset, *pair))
        .collect::<Vec<_>>();
    operand_entries.sort_unstable_by_key(|(offset, _)| *offset);
    for (offset, (lhs, rhs)) in operand_entries {
        write_u32_count("type map operand offset", offset, out)?;
        out.push(lhs as u8);
        out.push(rhs as u8);
    }
    Ok(())
}

fn write_optional_u32(value: Option<u32>, out: &mut Vec<u8>) {
    match value {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_le_bytes());
        }
        None => out.push(0),
    }
}

fn write_bool_slice(
    field: &'static str,
    values: &[bool],
    out: &mut Vec<u8>,
) -> Result<(), WireError> {
    write_u32_count(field, values.len(), out)?;
    out.extend(values.iter().map(|value| u8::from(*value)));
    Ok(())
}

fn write_optional_schema(schema: Option<&TypeSchema>, out: &mut Vec<u8>) -> Result<(), WireError> {
    match schema {
        Some(schema) => {
            out.push(1);
            write_schema(schema, out)?;
        }
        None => out.push(0),
    }
    Ok(())
}

fn write_optional_host_import_schema(
    schema: Option<&HostImportSchema>,
    out: &mut Vec<u8>,
) -> Result<(), WireError> {
    match schema {
        Some(schema) => {
            out.push(1);
            write_host_import_schema(schema, out)?;
        }
        None => out.push(0),
    }
    Ok(())
}

fn write_host_import_schema(schema: &HostImportSchema, out: &mut Vec<u8>) -> Result<(), WireError> {
    schema
        .validate()
        .map_err(|error| WireError::InvalidHostSchemaComplexity(error.to_string()))?;
    write_string("host import schema name", &schema.name, out)?;
    write_u32_count("host import schema parameters", schema.params.len(), out)?;
    for param in &schema.params {
        write_string("host import parameter name", &param.name, out)?;
        write_host_type_schema(&param.schema, out, 0)?;
        out.push(match param.passing {
            HostParamPassing::Value => 0,
            HostParamPassing::Borrow => 1,
            HostParamPassing::BorrowMut => 2,
            HostParamPassing::TakeOwned => 3,
        });
    }
    write_host_type_schema(&schema.return_type, out, 0)?;
    out.extend_from_slice(&schema.fingerprint.as_u64().to_le_bytes());
    Ok(())
}

fn write_host_type_schema(
    schema: &HostTypeSchema,
    out: &mut Vec<u8>,
    depth: usize,
) -> Result<(), WireError> {
    if depth >= MAX_HOST_SCHEMA_DEPTH {
        return Err(WireError::LengthTooLarge(
            "host schema nesting depth",
            depth,
        ));
    }
    match schema {
        HostTypeSchema::Unknown => out.push(0),
        HostTypeSchema::Null => out.push(1),
        HostTypeSchema::Int => out.push(2),
        HostTypeSchema::Float => out.push(3),
        HostTypeSchema::Number => out.push(4),
        HostTypeSchema::Bool => out.push(5),
        HostTypeSchema::String => out.push(6),
        HostTypeSchema::Bytes => out.push(7),
        HostTypeSchema::Array(inner) => {
            out.push(8);
            write_host_type_schema(inner, out, next_host_schema_depth(depth)?)?;
        }
        HostTypeSchema::Map(inner) => {
            out.push(9);
            write_host_type_schema(inner, out, next_host_schema_depth(depth)?)?;
        }
        HostTypeSchema::Optional(inner) => {
            out.push(10);
            write_host_type_schema(inner, out, next_host_schema_depth(depth)?)?;
        }
        HostTypeSchema::Callable { params, result } => {
            out.push(11);
            write_u32_count("host callable parameters", params.len(), out)?;
            for param in params {
                write_host_type_schema(param, out, depth + 1)?;
            }
            write_host_type_schema(result, out, depth + 1)?;
        }
        HostTypeSchema::Resource(key) => {
            out.push(12);
            write_string("host resource type key", key.as_str(), out)?;
        }
        HostTypeSchema::Named { name, fields } => {
            out.push(13);
            write_string("host named struct name", name, out)?;
            write_u32_count("host named struct fields", fields.len(), out)?;
            for field in fields {
                write_string("host named struct field name", &field.name, out)?;
                write_host_type_schema(&field.ty, out, next_host_schema_depth(depth)?)?;
            }
        }
    }
    Ok(())
}

fn next_host_schema_depth(depth: usize) -> Result<usize, WireError> {
    depth.checked_add(1).ok_or(WireError::LengthTooLarge(
        "host schema nesting depth",
        depth,
    ))
}

fn write_schema(schema: &TypeSchema, out: &mut Vec<u8>) -> Result<(), WireError> {
    match schema {
        TypeSchema::Unknown => out.push(0),
        TypeSchema::Null => out.push(1),
        TypeSchema::Int => out.push(2),
        TypeSchema::Float => out.push(3),
        TypeSchema::Number => out.push(4),
        TypeSchema::Bool => out.push(5),
        TypeSchema::String => out.push(6),
        TypeSchema::Bytes => out.push(7),
        TypeSchema::Optional(inner) => {
            out.push(16);
            write_schema(inner, out)?;
        }
        TypeSchema::GenericParam(name) => {
            out.push(8);
            write_string("schema generic", name, out)?;
        }
        TypeSchema::Named(name, type_args) => {
            out.push(9);
            write_string("schema name", name, out)?;
            write_u32_count("schema type args", type_args.len(), out)?;
            for type_arg in type_args {
                write_schema(type_arg, out)?;
            }
        }
        TypeSchema::Array(item) => {
            out.push(10);
            write_schema(item, out)?;
        }
        TypeSchema::ArrayTuple(items) => {
            out.push(11);
            write_u32_count("schema tuple items", items.len(), out)?;
            for item in items {
                write_schema(item, out)?;
            }
        }
        TypeSchema::ArrayTupleRest { prefix, rest } => {
            out.push(12);
            write_u32_count("schema tuple prefix", prefix.len(), out)?;
            for item in prefix {
                write_schema(item, out)?;
            }
            write_schema(rest, out)?;
        }
        TypeSchema::Map(item) => {
            out.push(13);
            write_schema(item, out)?;
        }
        TypeSchema::Object(fields) => {
            out.push(14);
            let mut entries = fields.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(lhs, _), (rhs, _)| lhs.cmp(rhs));
            write_u32_count("schema object fields", entries.len(), out)?;
            for (name, value) in entries {
                write_string("schema object field", name, out)?;
                write_schema(value, out)?;
            }
        }
        TypeSchema::Callable { params, result } => {
            out.push(15);
            write_u32_count("schema callable params", params.len(), out)?;
            for param in params {
                write_schema(param, out)?;
            }
            write_schema(result, out)?;
        }
        TypeSchema::Resource(key) => {
            out.push(17);
            write_string("schema resource key", key.as_str(), out)?;
        }
    }
    Ok(())
}

fn write_string(field: &'static str, value: &str, out: &mut Vec<u8>) -> Result<(), WireError> {
    write_u32_len(field, value.len(), out)?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_u32_len(field: &'static str, len: usize, out: &mut Vec<u8>) -> Result<(), WireError> {
    let len_u32 = u32::try_from(len).map_err(|_| WireError::LengthTooLarge(field, len))?;
    out.extend_from_slice(&len_u32.to_le_bytes());
    Ok(())
}

fn write_u32_count(field: &'static str, count: usize, out: &mut Vec<u8>) -> Result<(), WireError> {
    write_u32_len(field, count, out)
}
