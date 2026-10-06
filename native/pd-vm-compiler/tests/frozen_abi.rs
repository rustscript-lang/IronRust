//! Frozen core/pd-edge ABI 25 pin proofs and the checked-in example corpus.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::ptr;

use edge::{ABI_VERSION, function_by_name, host_namespace_specs};
use pd_vm_compiler::compile_edge_source_file;

const FROZEN_CORE_REV: &str = "b1d6cffede77f49410bf63525f30b9a46b02dc01";
const FROZEN_CORE_URL: &str = "https://github.com/rustscript-lang/rustscript.git";
const FROZEN_EDGE_REV: &str = "5f4f889e349bdfbd5534deb42bd13b616a6114f5";
const FROZEN_EDGE_URL: &str = "https://github.com/rustscript-lang/pd-edge.git";
const FROZEN_EDGE_ABI: u16 = 25;
const EXPECTED_EXAMPLES: usize = 7;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    manifest_dir().join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn git_lock_source(url: &str, rev: &str) -> String {
    format!("git+{url}?rev={rev}#{rev}")
}

fn collect_rss(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()))
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("rss") {
            out.push(path);
        }
    }
}

fn example_corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rss(&repo_root().join("examples"), &mut files);
    files.sort();
    files
}

fn compile_via_c_abi(path: &Path) -> Vec<u8> {
    let path_text = path.to_str().expect("utf-8 example path");
    let path_bytes = path_text.as_bytes();
    let mut output = ptr::null_mut();
    let mut length = 0;
    let status = pd_vm_compiler::pdvm_compile_file_utf8(
        path_bytes.as_ptr(),
        path_bytes.len(),
        &mut output,
        &mut length,
    );
    let diagnostic = if output.is_null() {
        String::new()
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(output, length) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    assert!(
        status == 0 && !output.is_null(),
        "{} native compile failed with status {status}: {diagnostic}",
        path.display()
    );
    let bytes = unsafe { std::slice::from_raw_parts(output, length) }.to_vec();
    pd_vm_compiler::pdvm_free_buffer(output, length);
    assert!(!bytes.is_empty(), "{} produced empty VMBC", path.display());
    bytes
}

#[test]
fn rustscript_and_pd_edge_are_pinned_to_the_frozen_full_shas() {
    let cargo_toml = read(&manifest_dir().join("Cargo.toml"));
    assert!(
        cargo_toml.contains(&format!("rev = \"{FROZEN_CORE_REV}\"")),
        "Cargo.toml must pin the frozen rustscript SHA"
    );
    assert!(
        cargo_toml.contains(&format!("git = \"{FROZEN_CORE_URL}\"")),
        "Cargo.toml must use the canonical rustscript HTTPS Git remote"
    );
    assert!(
        cargo_toml.contains(&format!("rev = \"{FROZEN_EDGE_REV}\"")),
        "Cargo.toml must pin the migrated pd-edge SHA"
    );
    assert!(
        cargo_toml.contains(&format!("git = \"{FROZEN_EDGE_URL}\"")),
        "Cargo.toml must use the canonical pd-edge HTTPS Git remote"
    );
    assert!(
        !cargo_toml.contains("path = \"../") && !cargo_toml.contains("path = '/"),
        "production crates must not use path pins"
    );
    assert!(
        !cargo_toml.contains("branch =") && !cargo_toml.contains("tag ="),
        "production crates must not use branch or tag pins"
    );
    assert!(
        !cargo_toml.contains("/home/") && !cargo_toml.contains("/mnt/"),
        "production crates must not use machine-specific paths"
    );
    assert_eq!(FROZEN_CORE_REV.len(), 40);
    assert_eq!(FROZEN_EDGE_REV.len(), 40);
    assert!(
        FROZEN_CORE_REV.chars().all(|ch| ch.is_ascii_hexdigit())
            && FROZEN_EDGE_REV.chars().all(|ch| ch.is_ascii_hexdigit()),
        "frozen pins must be full 40-character SHAs"
    );
}

#[test]
fn the_lockfile_proves_frozen_core_and_edge_sources() {
    let lock = read(&manifest_dir().join("Cargo.lock"));
    let expected_core = git_lock_source(FROZEN_CORE_URL, FROZEN_CORE_REV);
    let expected_edge = git_lock_source(FROZEN_EDGE_URL, FROZEN_EDGE_REV);
    let mut proven_core = BTreeSet::new();
    let mut proven_edge = BTreeSet::new();

    let mut lines = lock.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(name) = line
            .trim()
            .strip_prefix("name = \"")
            .and_then(|rest| rest.strip_suffix('"'))
        else {
            continue;
        };
        let version = lines.next().unwrap_or_default().trim().to_string();
        let source = lines.next().unwrap_or_default().trim().to_string();
        if !version.starts_with("version = ") {
            continue;
        }
        if source == format!("source = \"{expected_core}\"") {
            proven_core.insert(name.to_string());
        }
        if source == format!("source = \"{expected_edge}\"") {
            proven_edge.insert(name.to_string());
        }
    }

    for package in ["pd-vm", "pd-host-function", "pd-host-schema"] {
        assert!(
            proven_core.contains(package),
            "Cargo.lock must prove {package} at {expected_core}; core={proven_core:?}"
        );
    }
    assert!(
        proven_edge.contains("pd-edge-abi"),
        "Cargo.lock must prove pd-edge-abi at {expected_edge}; edge={proven_edge:?}"
    );
}

#[test]
fn compiler_dependencies_exclude_protocol_and_database_implementations() {
    let lock = read(&manifest_dir().join("Cargo.lock"));
    let packages = parse_lock_packages(&lock);
    for forbidden in [
        "pd-edge",
        "pd-edge-host-function",
        "tokio",
        "hyper",
        "axum",
        "rustls",
        "rusqlite",
        "libsqlite3-sys",
        "mimalloc",
        "aws-lc-rs",
    ] {
        assert!(
            !packages.iter().any(|package| package.name == forbidden),
            "compiler lockfile contains runtime implementation {forbidden}"
        );
    }
}

#[test]
fn every_locked_core_and_edge_revision_is_the_frozen_one() {
    let lock = read(&manifest_dir().join("Cargo.lock"));
    let packages = parse_lock_packages(&lock);
    assert!(
        packages.iter().any(|package| package.source.is_some()),
        "the lockfile must resolve at least one sourced dependency"
    );

    let mut saw_core = false;
    let mut saw_edge = false;
    for package in &packages {
        if !is_pd_family(&package.name) {
            continue;
        }
        if package.name == "pd-vm-compiler" {
            assert!(
                package.source.is_none(),
                "pd-vm-compiler must stay the local crate without a source"
            );
            continue;
        }

        let source = package.source.as_deref().unwrap_or("");
        assert!(
            !source.starts_with("registry+"),
            "{} {} must not resolve from crates.io: {source}",
            package.name,
            package.version
        );
        assert!(
            !source.starts_with("path+") && !source.contains("path="),
            "{} must not use a path pin: {source}",
            package.name
        );
        assert!(
            !source.contains("branch=") && !source.contains("tag="),
            "{} must not use a branch or tag pin: {source}",
            package.name
        );
        assert!(
            source.starts_with("git+"),
            "{} must lock a git source, got {source}",
            package.name
        );

        let rev = git_source_rev(source).unwrap_or("");
        assert!(
            rev.len() == 40 && rev.chars().all(|ch| ch.is_ascii_hexdigit()),
            "{} has a non-full SHA pin: {source}",
            package.name
        );

        if source.contains("rustscript.git") || source.contains("rustscript?") {
            saw_core = true;
            assert_eq!(
                rev, FROZEN_CORE_REV,
                "a stale core revision is locked for {}: {source}",
                package.name
            );
            assert!(
                source.contains(&format!("rev={FROZEN_CORE_REV}#{FROZEN_CORE_REV}")),
                "core source must use the full rev#sha form: {source}"
            );
        } else if source.contains("pd-edge.git") || source.contains("pd-edge?") {
            saw_edge = true;
            assert_eq!(
                rev, FROZEN_EDGE_REV,
                "a stale edge revision is locked for {}: {source}",
                package.name
            );
            assert!(
                source.contains(&format!("rev={FROZEN_EDGE_REV}#{FROZEN_EDGE_REV}")),
                "edge source must use the full rev#sha form: {source}"
            );
        } else {
            panic!(
                "{} must come from frozen rustscript or pd-edge, got {source}",
                package.name
            );
        }
    }
    assert!(saw_core, "Cargo.lock must lock the frozen rustscript core");
    assert!(saw_edge, "Cargo.lock must lock the migrated pd-edge");
}

struct LockedPackage {
    name: String,
    version: String,
    source: Option<String>,
}

fn parse_lock_packages(lock: &str) -> Vec<LockedPackage> {
    let mut packages = Vec::new();
    let mut name = None;
    let mut version = None;
    let mut source = None;

    let flush = |packages: &mut Vec<LockedPackage>,
                 name: &mut Option<String>,
                 version: &mut Option<String>,
                 source: &mut Option<String>| {
        if let Some(name) = name.take() {
            packages.push(LockedPackage {
                name,
                version: version.take().unwrap_or_default(),
                source: source.take(),
            });
        } else {
            version.take();
            source.take();
        }
    };

    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            flush(&mut packages, &mut name, &mut version, &mut source);
            continue;
        }
        if line.starts_with('[') {
            flush(&mut packages, &mut name, &mut version, &mut source);
            continue;
        }
        if let Some(value) = line
            .strip_prefix("name = \"")
            .and_then(|rest| rest.strip_suffix('"'))
        {
            name = Some(value.to_string());
        } else if let Some(value) = line
            .strip_prefix("version = \"")
            .and_then(|rest| rest.strip_suffix('"'))
        {
            version = Some(value.to_string());
        } else if let Some(value) = line
            .strip_prefix("source = \"")
            .and_then(|rest| rest.strip_suffix('"'))
        {
            source = Some(value.to_string());
        }
    }
    flush(&mut packages, &mut name, &mut version, &mut source);
    packages
}

fn is_pd_family(name: &str) -> bool {
    name == "pd-vm"
        || name == "pd-vm-compiler"
        || name.starts_with("pd-vm-")
        || name == "pd-edge"
        || name.starts_with("pd-edge")
        || name.starts_with("pd-host-")
}

fn git_source_rev(source: &str) -> Option<&str> {
    let after_rev = source.split("rev=").nth(1)?;
    after_rev.split(['#', '&']).next()
}

#[test]
fn ci_resolves_pins_from_cargo_and_does_not_use_sibling_checkouts() {
    let workflow = read(&repo_root().join(".github/workflows/ci.yml"));
    assert!(
        !workflow.contains("repository: rustscript-lang/rustscript"),
        "CI must resolve rustscript from the Cargo git pin"
    );
    assert!(
        !workflow.contains("repository: rustscript-lang/pd-edge"),
        "CI must resolve pd-edge from the Cargo git pin"
    );
}

#[test]
fn edge_abi_is_version_25_with_typed_http_catalog_and_named_mqtt_event_surface() {
    assert_eq!(ABI_VERSION, FROZEN_EDGE_ABI);
    let namespaces: Vec<&str> = host_namespace_specs()
        .iter()
        .map(|spec| spec.root)
        .collect();
    for required in ["http", "proxy"] {
        assert!(
            namespaces.contains(&required),
            "production catalog missing namespace {required}: {namespaces:?}"
        );
    }

    let manifest = edge::abi_json();
    assert!(
        manifest.contains("\"abi_version\": 25"),
        "published ABI JSON must record version 25"
    );

    if let Some(function) = function_by_name("mqtt::connection::next_event") {
        assert!(
            function.docs.contains("MqttEvent") || function.return_type.as_str() != "unknown",
            "mqtt::connection::next_event must stay a named MqttEvent boundary: {}",
            function.docs
        );
    }
    if namespaces.contains(&"mqtt") {
        assert!(
            manifest.contains("MqttEvent"),
            "mqtt-enabled catalog must publish named MqttEvent"
        );
    }
}

#[test]
fn example_corpus_has_the_exact_checked_in_count() {
    let corpus = example_corpus();
    assert_eq!(
        corpus.len(),
        EXPECTED_EXAMPLES,
        "example corpus count drifted: {corpus:?}"
    );
}

#[test]
fn native_compiler_compiles_and_executes_the_non_overlay_example_corpus() {
    let corpus = example_corpus();
    assert_eq!(corpus.len(), EXPECTED_EXAMPLES);

    let mut compiled = Vec::new();
    for path in &corpus {
        let source = read(path);
        if source.contains("use System::") {
            continue;
        }
        let vmbc = compile_via_c_abi(path);
        let program = vm::decode_program(&vmbc)
            .unwrap_or_else(|error| panic!("{} VMBC decode failed: {error}", path.display()));
        assert_eq!(
            vmbc,
            vm::encode_program(&program).expect("frozen upstream encoding"),
            "{} compiler-only encoding must match upstream byte for byte",
            path.display()
        );
        assert_eq!(
            u16::from_le_bytes(vmbc[4..6].try_into().expect("VMBC version bytes")),
            13,
            "{} must emit VMBC v13",
            path.display()
        );
        compiled.push((path.clone(), program, source));
    }
    assert_eq!(
        compiled.len(),
        4,
        "compile-smoke plus the three pd-edge HTTP examples must compile through the native compiler"
    );

    for (path, program, source) in compiled {
        if path.file_name().and_then(|name| name.to_str()) == Some("compile-smoke.rss") {
            let mut runtime = vm::Vm::new(program);
            let status = runtime
                .run()
                .unwrap_or_else(|error| panic!("{} failed to execute: {error}", path.display()));
            assert_eq!(status, vm::VmStatus::Halted);
            continue;
        }

        let compiled = compile_edge_source_file(&path).unwrap_or_else(|error| {
            panic!("{} edge catalog compile failed: {error}", path.display())
        });
        let imports: Vec<&str> = compiled
            .program
            .imports
            .iter()
            .map(|import| import.name.as_str())
            .collect();
        assert!(
            !compiled.program.host_import_schemas().is_empty(),
            "{} must carry typed host schemas/fingerprints",
            path.display()
        );
        if source.contains("http::response::set_status") {
            assert!(
                imports.contains(&"http::response::set_status"),
                "{} missing http::response::set_status: {imports:?}",
                path.display()
            );
        }
        for import in &compiled.program.imports {
            if function_by_name(&import.name).is_some() {
                assert!(
                    compiled
                        .program
                        .host_import_schemas()
                        .iter()
                        .flatten()
                        .any(|schema| schema.name == import.name),
                    "{} catalog import {} missing schema",
                    path.display(),
                    import.name
                );
            }
        }
    }
}

#[test]
fn unknown_host_namespaces_fail_closed_without_an_accepted_error_skip() {
    let tmp = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir().join("target"))
        .join("ironrust-unknown-host");
    std::fs::create_dir_all(&tmp).expect("scratch directory");
    let path = tmp.join("unknown.rss");
    std::fs::write(&path, "use http;\nhttp::response::set_status();\n").expect("write probe");
    let path_bytes = path.to_str().expect("utf-8").as_bytes();
    let mut output = ptr::null_mut();
    let mut length = 0;
    let status = pd_vm_compiler::pdvm_compile_file_utf8(
        path_bytes.as_ptr(),
        path_bytes.len(),
        &mut output,
        &mut length,
    );
    assert_ne!(status, 0, "unknown host namespaces must fail closed");
    let message = if output.is_null() {
        String::new()
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(output, length) };
        let text = String::from_utf8_lossy(bytes).into_owned();
        pd_vm_compiler::pdvm_free_buffer(output, length);
        text
    };
    assert!(
        !message.is_empty(),
        "fail-closed diagnostics must not be empty"
    );
    assert!(
        !message.contains("/home/wow"),
        "diagnostics must not mention a machine-specific path: {message}"
    );
}
