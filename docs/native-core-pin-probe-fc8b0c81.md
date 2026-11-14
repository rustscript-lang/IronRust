# Native core pin probe: `pd-vm` b1d6cffe → fc8b0c81

Probe only. The pin bump is applied and the native crate compiles, but the migration is
**not mergeable as-is**: the VMBC wire format version moves from 13 to 14, and the edge ABI
crate has no revision that matches the new core. Details and the exact remaining work below.

## 1. Pin and lockfile

`native/pd-vm-compiler/Cargo.toml` — both the `[dependencies]` and `[dev-dependencies]`
`vm` entries moved from `b1d6cffe…` to `fc8b0c81…`. The `edge` (`pd-edge-abi`) pin is
unchanged at `5f4f889e…`.

`cargo update -p pd-vm --precise fc8b0c8179640a8ffe460a64b3690e35c2f7eb73` also moved
`pd-host-function` and `pd-host-schema`, so the lockfile is **+27 / −7**, not a two-line diff.

Collateral change: the lock now carries **two** `pd-host-function` and **two**
`pd-host-schema` packages, because `pd-edge-abi@5f4f889` hard-pins
`pd-host-function = { rev = "b1d6cffe…" }`. Cargo also recorded one of them with a
self-inconsistent source string (`?rev=b1d6cffe…#fc8b0c81…`), which is what
`every_locked_core_and_edge_revision_is_the_frozen_one` trips over.

## 2. Compile surface (`cargo check --locked --all-targets`)

Exit 101, exactly **two** errors, both `E0004` non-exhaustive `match` on new enum variants
added by core:

| Site | Enum | New variants |
| --- | --- | --- |
| `src/wire.rs:410` | `HostTypeSchema` (`vm::host_api`) | `Unit`, `Option(_)`, `Result { ok, err }` |
| `src/wire.rs:464` | `TypeSchema` (`vm::compiler::ir`) | `Unit`, `Option(_)`, `Result { ok, err }` |

Nothing in the failure was an edge-ABI or symbol problem, and no `pd-vm` API that IronRust
uses was removed or renamed.

Minimal fix applied (upstream tag values read from core at `fc8b0c81`):

| Variant | `HostTypeSchema` tag | `TypeSchema` tag |
| --- | --- | --- |
| `Unit` | 16 | 20 |
| `Result { ok, err }` | 14 | 18 |
| `Option(_)` | 15 | 19 |

After that: `cargo check --locked --all-targets` → **0**, `cargo fmt --check` → **0**,
`cargo clippy --locked --all-targets -- -D warnings` → **0**.

## 3. Wire format actually changed: VMBC v13 → v14

`cargo test --locked` still fails on byte parity. The delta is not the schema tags — it is the
container version plus one new trailing section:

- header version field: `13` → `14` (`VERSION_V14`);
- new trailing section `write_ret_cleanups` (after `write_named_struct_decls`):
  `u32` plan count, then per plan `u32 site`, `u32 local_count`, and per local
  `u16 slot` + `write_schema`.

Minimal reproducer diff (`matches_upstream_encoder_for_normalized_programs`): identical bytes
except offset 4 (`13`→`14`) and 4 extra trailing zero bytes (the empty plan count).

Test state at this commit:

- `--lib`: 3 failed — `matches_upstream_encoder_for_normalized_programs`,
  `nested_constants_match_the_frozen_encoder`,
  `pinned_rust_runtime_matches_shared_callable_parity_fixture` (all version/`ret_cleanups`);
- `--test frozen_abi`: 4 failed — `rustscript_and_pd_edge_are_pinned_to_the_frozen_full_shas`
  and `the_lockfile_proves_frozen_core_and_edge_sources` (expected: frozen constants still say
  `b1d6cffe`), `every_locked_core_and_edge_revision_is_the_frozen_one` (the duplicate
  `pd-host-function` entry, see §4),
  `native_compiler_compiles_and_executes_the_non_overlay_example_corpus` (decodes v14).

Completing this needs `write_ret_cleanups` **plus** its fail-closed validation
(`validate_ret_cleanups`, `ret_instruction_sites`, `cleanup_frame_local_count`,
`require_owned_cleanup_schema`, and a new `WireError::InvalidRetCleanup`), roughly 180 lines
mirroring `src/vmbc.rs` at `fc8b0c81`. The types are public (`vm::{OpCode, RetCleanupPlan,
RetCleanupLocal, CallableTarget, …}`, `OpCode::operand_len`), so it is portable, but the port
cannot be replaced by a call: `vm::vmbc` is `#[cfg(feature = "runtime")]` and this crate builds
the encoder by hand on purpose.

## 4. `edge-abi` compatibility: the real blocker

`edge-abi` still exists at `fc8b0c81` and is unchanged: `edge-abi = ["dep:edge_abi",
"edge_abi/console", "edge_abi/http", "edge_abi/http2", "edge_abi/mqtt", "edge_abi/tls",
"edge_abi/websocket", "edge_abi/webrtc"]` with
`edge_abi = { package = "pd-edge-abi", version = "0.1.1", … }` (crates.io), exactly as at
`b1d6cffe`. IronRust does **not** enable that feature — `vm` is built with
`default-features = false` and no `edge-abi` — so the crates.io `pd-edge-abi 0.1.1`
(ABI 24, per the warning comment in pd-edge's own `Cargo.toml`) never enters this graph.
The ABI surface this crate actually compiles against is the direct git pin `pd-edge-abi@5f4f889`
(ABI 25), which also compiles unchanged.

The gap is a `pd-host-function` revision split, not an ABI symbol mismatch:

- `pd-edge-abi@5f4f889` (ABI 25) pins `pd-host-function` at `b1d6cffe`;
- core at `fc8b0c81` has its own `pd-host-function` / `pd-host-schema`.

Two git checkouts of the same crate in one graph is precisely what the frozen-pin tests forbid,
so the invariant cannot be satisfied at any native-side cost. **No pd-edge revision fixes this
today**: `pd-edge` `master` (`c8986725`) still declares `ABI_VERSION = 25` and still pins
`pd-host-function` at `b1d6cffe`. There is no ABI 26 edge crate yet, published or on `master`.

Required before this pin can land: a pd-edge revision (ABI 26) whose own
`pd-host-function` / `pd-host-schema` pins point at the new core rev — i.e. work on the pd-edge
line, not here. `pd-edge-abi 0.1.1` also needs republishing if the workspace `edge-abi` feature
is ever enabled.

## 5. .NET side: not verified, and it will reject v14

No .NET SDK on this host (`dotnet: command not found`), so nothing managed was built or tested.
Reading the source, the v14 payload breaks the managed reader:

- `PdVm.Compiler/PdVmVmbcReader.cs:11` — `private const ushort Version = 13;`, rejected outright
  at line 39;
- line 88 throws on any trailing byte, so the new `ret_cleanups` section must be parsed or
  skipped explicitly;
- `ReadHostTypeSchema` / `ReadSchema` need the same six new tags (14/15/16, 18/19/20) that
  `wire.rs` just gained, or any payload carrying `Unit`/`Option`/`Result` schemas throws.

`PdVm.Runtime/PdVmBuiltins.cs:585` and `PdVm.Runner/Program.cs:165` also carry "VMBC v13"
wording that would need revisiting.

## 6. Recommendation

Do not merge the pin on its own. Sequence:

1. **pd-edge line first**: cut ABI 26 with `pd-host-function`/`pd-host-schema` pinned to the
   target core rev, then repoint `edge` here. Without this the lockfile cannot be canonical and
   the frozen-pin tests cannot pass.
2. **Finish the native encoder**: port `write_ret_cleanups` + validation, `VERSION_V14`, and
   bump `FROZEN_CORE_REV` / the "must emit VMBC v14" assertion in `tests/frozen_abi.rs`.
3. **Managed reader**: `PdVmVmbcReader` v14 + `ret_cleanups` + the six schema tags, verified on
   a host with the .NET SDK before the pin lands.

Until 1–3 are done this branch is probe-only evidence: the native crate compiles and passes
fmt/clippy at the new pin, and the remaining gap is fully enumerated above.
