param([string] $RustTarget)

$ErrorActionPreference = 'Stop'
$manifest = Join-Path $PSScriptRoot '../native/pd-vm-compiler/Cargo.toml'
$cargoArgs = @('tree', '--locked', '--manifest-path', $manifest,
    '--edges', 'normal,build', '--prefix', 'none', '--format', '{p} features={f}')
if ($RustTarget) {
    $cargoArgs += @('--target', $RustTarget)
}
$dependencyLines = & cargo @cargoArgs
if ($LASTEXITCODE -ne 0) {
    throw 'Unable to inspect native compiler production dependencies'
}
foreach ($line in $dependencyLines) {
    if ($line -match '^pd-vm v.*features=(.*)$') {
        $features = $Matches[1].Split(',')
        foreach ($feature in @('runtime', 'async', 'http-client', 'sqlite', 'cli', 'cranelift-jit', 'edge-abi')) {
            if ($features -contains $feature) {
                throw "Native compiler must not enable pd-vm/$feature"
            }
        }
    }
    if ($line -match '^(pd-edge|pd-edge-host-function|tokio|hyper|hyper-util|axum|rustls|tokio-rustls|rusqlite|libsqlite3-sys|reqwest|mimalloc|aws-lc-rs|cranelift-[\w-]+) v') {
        throw "Runtime dependency in native compiler: $line"
    }
}
Write-Host 'Native compiler production graph contains no Rust VM, HTTP/TLS stack, SQLite, or JIT.'
