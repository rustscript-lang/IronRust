# IronRust

IronRust provides a RustScript runtime and compiler for Microsoft CLR.

## Quick start

```powershell
cargo build --locked --release --manifest-path native/pd-vm-compiler/Cargo.toml
dotnet build IronRust.sln --configuration Release
```

Rebuild the native compiler after updating the repository. Source compilation
requires its VMBC format to match the CLR reader.

On Windows, launch the Minesweeper example with:

```powershell
.\run-minesweeper.bat
```

Run the native compiler and CLR tests with:

```powershell
cargo test --locked --manifest-path native/pd-vm-compiler/Cargo.toml
dotnet test IronRust.sln --configuration Release
```

## Documentation

- [IronRust reference](https://rustscript.org/docs/reference/ironrust/)
- [Runtime guides](https://rustscript.org/docs/learn/runtimes/#ironrust)
- [Runtime implementation guide](https://rustscript.org/docs/contribute/runtimes/#ironrust)
