using PdVm.Compiler;
using PdVm.Runtime;

namespace PdVm.Tests;

public sealed class PdVmAbi25ExampleMatrixTests
{
    private const int ExpectedExampleCount = 7;

    [Fact]
    public void CheckedInExamplesHaveExactCountAndCompileThroughNativeCompilerAndClr()
    {
        var examples = Directory.GetFiles(ExamplesRoot(), "*.rss")
            .OrderBy(path => path, StringComparer.Ordinal)
            .ToArray();
        Assert.Equal(ExpectedExampleCount, examples.Length);

        var compiled = 0;
        var executed = 0;
        var overlaySkipped = 0;
        foreach (var path in examples)
        {
            var source = File.ReadAllText(path);
            var name = Path.GetFileName(path);
            if (source.Contains("use System::", StringComparison.Ordinal))
            {
                if (!OperatingSystem.IsWindows() &&
                    (name.Contains("winforms", StringComparison.OrdinalIgnoreCase) ||
                     name.Contains("minesweeper", StringComparison.OrdinalIgnoreCase)))
                {
                    overlaySkipped++;
                    continue;
                }

                var outputPath = Path.Combine(ScratchRoot(), $"{Guid.NewGuid():N}.dll");
                PdVmDotNetSourceCompiler.CompileFile(path, outputPath);
                var program = PdVmAssemblyLoader.LoadProgram(outputPath);
                compiled++;
                if (name == "dotnet-typed-console.rss")
                {
                    var result = PdVmExecution.Run(program, PdVmDefaultHost.CreateConsoleHost());
                    Assert.Equal(PdVmStatusKind.Halted, result.Status.Kind);
                    executed++;
                }

                continue;
            }

            var bytes = PdVmNativeCompiler.CompileFile(path);
            Assert.True(bytes.AsSpan(0, 4).SequenceEqual("VMBC"u8));
            Assert.Equal((ushort)13, BitConverter.ToUInt16(bytes, 4));
            var model = PdVmVmbcReader.ReadBytes(bytes);
            Assert.NotEmpty(model.Code);
            if (source.Contains("http::", StringComparison.Ordinal) ||
                source.Contains("use http", StringComparison.Ordinal))
            {
                Assert.Contains(model.Imports, import => import.Name.StartsWith("http::", StringComparison.Ordinal));
                Assert.Contains(model.HostImportSchemas, schema => schema is { Fingerprint: not 0 });
            }

            var clrOutput = Path.Combine(ScratchRoot(), $"{Guid.NewGuid():N}.dll");
            PdVmClrCompiler.Compile(
                bytes,
                clrOutput,
                new PdVmCompileOptions
                {
                    AssemblyName = $"PdVm.Example.{Guid.NewGuid():N}",
                    TypeName = $"PdVm.Example.Program_{Guid.NewGuid():N}",
                });
            var clrProgram = PdVmAssemblyLoader.LoadProgram(clrOutput);
            compiled++;
            if (name == "compile-smoke.rss")
            {
                var result = PdVmExecution.Run(clrProgram, PdVmDefaultHost.CreateConsoleHost());
                Assert.Equal(PdVmStatusKind.Halted, result.Status.Kind);
                executed++;
            }
        }

        Assert.Equal(ExpectedExampleCount, compiled + overlaySkipped);
        Assert.True(compiled >= 5, $"expected at least five examples to compile, got {compiled}");
        Assert.True(executed >= 1, "compile-smoke must execute through the migrated CLR runtime");
    }

    private static string ExamplesRoot() =>
        Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "examples"));

    private static string ScratchRoot()
    {
        var root = Path.Combine(Path.GetTempPath(), "ironrust-abi25-examples");
        Directory.CreateDirectory(root);
        return root;
    }
}
