using System.Reflection;
using System.Text.RegularExpressions;
using PdVm.Compiler;
using PdVm.Runtime;

namespace PdVm.Tests;

public sealed class PdVmMinesweeperTests
{
    [Fact]
    public void GameCallbacksPreserveBoardStateAcrossClicksResetAndDifficultyChanges()
    {
        if (!OperatingSystem.IsWindows())
        {
            return;
        }

        var example = Path.GetFullPath(Path.Combine(
            AppContext.BaseDirectory, "..", "..", "..", "..", "examples", "dotnet-minesweeper.rss"));
        var root = Path.Combine(Path.GetTempPath(), "pd-vm-minesweeper-tests", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            // Export the existing callback bodies so assertions exercise the checked-in game.
            var source = Regex.Replace(
                File.ReadAllText(example).Replace(
                    "Random::NewRandom()", "Random::NewRandomInt32(12345)", StringComparison.Ordinal),
                @"(?m)^fn (reset_game|on_beginner|on_intermediate|on_expert|on_pause|on_board_down|on_board_up|on_board_double_click|on_timer_tick|on_exit)\(",
                "pub fn $1(");
            source += """

                pub fn test_board() -> [int] {
                    let game = &state;
                    let current_mines = &mines;
                    let current_revealed = &revealed;
                    let current_flagged = &flagged;
                    let current_display = &display;
                    let current_flash = &flash_cells;
                    [(&game).rows, (&game).columns, (&game).mine_total,
                     (&game).flag_count, (&game).safe_remaining,
                     current_mines.length, current_revealed.length, current_flagged.length,
                     current_display.length, current_flash.length]
                }
                pub fn test_started() -> bool { let game = &state; (&game).started }
                pub fn test_active() -> bool { let game = &state; (&game).active }
                pub fn test_paused() -> bool { let game = &state; (&game).paused }
                pub fn test_cell(index: int) -> [int] {
                    let current_mines = &mines;
                    let current_revealed = &revealed;
                    let current_flagged = &flagged;
                    let current_display = &display;
                    [(&current_mines)[index], (&current_revealed)[index],
                     (&current_flagged)[index], (&current_display)[index]]
                }
                """;
            var sourcePath = Path.Combine(root, "minesweeper.rss");
            var outputPath = Path.Combine(root, "minesweeper.dll");
            File.WriteAllText(sourcePath, source);
            PdVmDotNetSourceCompiler.CompileFile(
                sourcePath, outputPath,
                new PdVmDotNetSourceCompileOptions { Profile = PdVmDotNetInteropProfile.WindowsForms });

            Exception? failure = null;
            var thread = new Thread(() =>
            {
                try
                {
                    PdVmDotNetHost.InitializeWindowsFormsApplication();
                    using var program = Assert.IsAssignableFrom<IPdVmCallableProgram>(
                        PdVmAssemblyLoader.CreateProgram(Assembly.Load(File.ReadAllBytes(outputPath))));
                    var host = PdVmDefaultHost.CreateConsoleHost();
                    host.RegisterFallback(new PdVmDotNetHost().Call);
                    using var application = PdVmWinFormsApplication.Attach(program, host);
                    Assert.Equal(PdVmStatusKind.Halted, PdVmExecution.Run(program, host).Status.Kind);
                    Assert.True(application.HasMainForm);
                    PdVmValue Invoke(string name, params PdVmValue[] args) =>
                        program.InvokeCallableAsync(program.ResolveCallable(name), args, host)
                            .GetAwaiter().GetResult();
                    long[] Board() => Invoke("test_board").AsArray().Select(value => value.AsInt()).ToArray();
                    long[] Cell(int index) => Invoke("test_cell", PdVmValue.FromInt(index))
                        .AsArray().Select(value => value.AsInt()).ToArray();
                    PdVmValue Pointer(int index, string button = "Left")
                    {
                        var columns = Board()[1];
                        return PdVmValue.FromMap([
                            new(PdVmValue.FromString("button"), PdVmValue.FromString(button)),
                            new(PdVmValue.FromString("x"), PdVmValue.FromString((index % columns * 32 + 3).ToString())),
                            new(PdVmValue.FromString("y"), PdVmValue.FromString((index / columns * 32 + 3).ToString())),
                        ]);
                    }
                    void Click(int index)
                    {
                        Invoke("on_board_down", Pointer(index));
                        Invoke("on_board_up", Pointer(index));
                    }
                    void AssertBoard(int rows, int columns, int mines)
                    {
                        Assert.Equal([rows, columns, mines, 0L, rows * columns - mines,
                            rows * columns, rows * columns, rows * columns, rows * columns, rows * columns], Board());
                        Assert.False(Invoke("test_started").AsBool());
                        Assert.True(Invoke("test_active").AsBool());
                    }

                    try
                    {
                        AssertBoard(8, 8, 10);
                        Invoke("on_board_up", Pointer(1, "Right"));
                        Assert.Equal(1, Cell(1)[2]);
                        Assert.Equal(1, Board()[3]);
                        Invoke("on_board_up", Pointer(1, "Right"));
                        Assert.Equal(2, Cell(1)[2]);
                        Assert.Equal(0, Board()[3]);
                        Invoke("on_board_up", Pointer(1, "Right"));
                        Assert.Equal(0, Cell(1)[2]);

                        Click(0);
                        Assert.True(Invoke("test_started").AsBool());
                        Assert.True(Invoke("test_active").AsBool());
                        Assert.Equal(0, Cell(0)[0]);
                        Assert.Equal(1, Cell(0)[1]);
                        Assert.Equal(10, Enumerable.Range(0, 64).Sum(index => Cell(index)[0]));
                        Invoke("on_pause");
                        Assert.True(Invoke("test_paused").AsBool());
                        var pausedBoard = Board();
                        Click(1);
                        Invoke("on_timer_tick");
                        Assert.Equal(pausedBoard, Board());
                        Invoke("on_pause");
                        Assert.False(Invoke("test_paused").AsBool());

                        // Reveal a numbered safe cell, then flag its adjacent mines before chording.
                        var numbered = Enumerable.Range(0, 64).First(index =>
                            Cell(index)[0] == 0 && Neighbors(index, 8, 8).Any(neighbor => Cell(neighbor)[0] == 1));
                        Click(numbered);
                        foreach (var neighbor in Neighbors(numbered, 8, 8).Where(index => Cell(index)[0] == 1))
                        {
                            Invoke("on_board_up", Pointer(neighbor, "Right"));
                        }
                        Invoke("on_board_double_click", Pointer(numbered));
                        Assert.Equal(Board()[4] > 0, Invoke("test_active").AsBool());
                        foreach (var neighbor in Neighbors(numbered, 8, 8).Where(index => Cell(index)[0] == 0))
                        {
                            Assert.Equal(1, Cell(neighbor)[1]);
                        }

                        foreach (var index in Enumerable.Range(0, 64).Where(index => Cell(index)[0] == 0))
                        {
                            Click(index);
                        }
                        Assert.Equal(0, Board()[4]);
                        Assert.False(Invoke("test_active").AsBool());
                        Invoke("reset_game");
                        AssertBoard(8, 8, 10);
                        Invoke("on_intermediate");
                        AssertBoard(16, 16, 40);
                        Click(0);
                        Assert.Equal(1, Cell(0)[1]);
                        Invoke("on_expert");
                        AssertBoard(16, 30, 99);
                        Click(479);
                        Assert.Equal(1, Cell(479)[1]);
                        Invoke("on_beginner");
                        AssertBoard(8, 8, 10);
                    }
                    finally
                    {
                        Invoke("on_exit");
                    }
                }
                catch (Exception exception)
                {
                    failure = exception;
                }
            }) { IsBackground = true };
            thread.SetApartmentState(ApartmentState.STA);
            thread.Start();
            Assert.True(thread.Join(TimeSpan.FromSeconds(30)), "Minesweeper callbacks exceeded the test deadline");
            Assert.Null(failure);
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    private static IEnumerable<int> Neighbors(int index, int rows, int columns)
    {
        for (var rowDelta = -1; rowDelta <= 1; rowDelta++)
        {
            for (var columnDelta = -1; columnDelta <= 1; columnDelta++)
            {
                var row = index / columns + rowDelta;
                var column = index % columns + columnDelta;
                if ((rowDelta != 0 || columnDelta != 0) && row >= 0 && row < rows && column >= 0 && column < columns)
                {
                    yield return row * columns + column;
                }
            }
        }
    }
}
