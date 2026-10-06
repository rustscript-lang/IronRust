//! Compiler-only projection of the frozen pd-edge ABI25 declarations.
//! Schema projection and stdlib overlays follow pd-edge 5f4f889
//! (see LICENSE.pd-edge); there are no protocol handlers or VM adapters.
use edge::{AbiParamType, AbiValueType};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use vm::{
    CompileSourceFileOptions, CompiledProgram, HostApiBuilder, HostApiCatalog, HostFunctionSchema,
    HostParamSchema, HostStructField, HostTypeSchema, SourcePathError,
};

fn edge_param_type_schema(param: AbiParamType) -> HostTypeSchema {
    match param {
        AbiParamType::Any => HostTypeSchema::Unknown,
        AbiParamType::Null => HostTypeSchema::Null,
        AbiParamType::Int => HostTypeSchema::Int,
        AbiParamType::Float => HostTypeSchema::Float,
        AbiParamType::Bool => HostTypeSchema::Bool,
        AbiParamType::String => HostTypeSchema::String,
        AbiParamType::Bytes => HostTypeSchema::Bytes,
        AbiParamType::Array => HostTypeSchema::Array(Box::new(HostTypeSchema::Unknown)),
        AbiParamType::Map => HostTypeSchema::Map(Box::new(HostTypeSchema::Unknown)),
        AbiParamType::Number => HostTypeSchema::Number,
    }
}

fn edge_return_type_schema(value: AbiValueType) -> HostTypeSchema {
    match value {
        AbiValueType::Unknown => HostTypeSchema::Unknown,
        AbiValueType::Null => HostTypeSchema::Null,
        AbiValueType::Int => HostTypeSchema::Int,
        AbiValueType::Float => HostTypeSchema::Float,
        AbiValueType::Bool => HostTypeSchema::Bool,
        AbiValueType::String => HostTypeSchema::String,
        AbiValueType::Bytes => HostTypeSchema::Bytes,
        AbiValueType::Array => HostTypeSchema::Array(Box::new(HostTypeSchema::Unknown)),
        AbiValueType::Map => HostTypeSchema::Map(Box::new(HostTypeSchema::Unknown)),
    }
}

fn mqtt_event_schema() -> HostTypeSchema {
    HostTypeSchema::named_struct(
        "MqttEvent",
        vec![
            HostStructField::new("kind", HostTypeSchema::String),
            HostStructField::new(
                "topic",
                HostTypeSchema::Optional(Box::new(HostTypeSchema::String)),
            ),
            HostStructField::new(
                "payload_text",
                HostTypeSchema::Optional(Box::new(HostTypeSchema::String)),
            ),
            HostStructField::new(
                "payload_base64",
                HostTypeSchema::Optional(Box::new(HostTypeSchema::String)),
            ),
            HostStructField::new(
                "qos",
                HostTypeSchema::Optional(Box::new(HostTypeSchema::Int)),
            ),
            HostStructField::new(
                "retain",
                HostTypeSchema::Optional(Box::new(HostTypeSchema::Bool)),
            ),
            HostStructField::new(
                "dup",
                HostTypeSchema::Optional(Box::new(HostTypeSchema::Bool)),
            ),
            HostStructField::new(
                "reason",
                HostTypeSchema::Optional(Box::new(HostTypeSchema::String)),
            ),
        ],
    )
}

fn overlay_named_return(name: &str, fallback: HostTypeSchema) -> HostTypeSchema {
    match name {
        "mqtt::connection::read_event" => mqtt_event_schema(),
        _ => fallback,
    }
}

fn edge_function_schema(name: &str) -> Result<HostFunctionSchema, String> {
    let function = edge::function_by_name(name).ok_or_else(|| {
        format!("edge host function '{name}' is not declared in the edge ABI spec")
    })?;
    if function.param_names.len() != function.param_types.len() {
        return Err(format!(
            "edge ABI function '{name}' declares {} parameter names for {} parameter types",
            function.param_names.len(),
            function.param_types.len()
        ));
    }
    Ok(HostFunctionSchema {
        name: function.name.to_string(),
        params: function
            .param_names
            .iter()
            .zip(function.param_types.iter())
            .map(|(param_name, param_type)| {
                HostParamSchema::value(*param_name, edge_param_type_schema(*param_type))
            })
            .collect(),
        return_type: overlay_named_return(
            function.name,
            edge_return_type_schema(function.return_type),
        ),
        description: function.docs.to_string(),
    })
}

fn catalog() -> Arc<HostApiCatalog> {
    static CATALOG: OnceLock<Arc<HostApiCatalog>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            let mut builder = HostApiBuilder::new();
            for function in edge::FUNCTIONS {
                let schema =
                    edge_function_schema(function.name).expect("valid frozen ABI function");
                builder.function(schema);
            }
            Arc::new(builder.build().expect("valid frozen ABI catalog"))
        })
        .clone()
}

pub fn compile_edge_source_file(
    path: impl AsRef<Path>,
) -> Result<CompiledProgram, SourcePathError> {
    let mut options = CompileSourceFileOptions::new();
    options.set_host_api_catalog(catalog());
    options.set_module_override_source(
        "edge/http/upstream/request.rss",
        include_str!("../stdlib/http/upstream/request.rss"),
    );
    options.set_module_override_source(
        "edge/http/upstream/response.rss",
        include_str!("../stdlib/http/upstream/response.rss"),
    );
    options.set_module_override_source(
        "edge/http/upstream.rss",
        include_str!("../stdlib/http/upstream.rss"),
    );
    vm::compile_source_file_with_options(path, options)
}
