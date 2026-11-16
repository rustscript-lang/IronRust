using System.Reflection;
using PdVm.Runtime;

namespace PdVm.Tests;

/// <summary>
/// Windows Forms keeps process-wide state: visual styles and DPI awareness can only be
/// configured before the first window handle exists, and <c>Application.Exit</c> terminates
/// every message loop in the process. Tests that create forms therefore join one
/// non-parallel collection and share a single initialization.
/// </summary>
[CollectionDefinition(Name, DisableParallelization = true)]
public sealed class WinFormsCollection
{
    public const string Name = "windows-forms";
}

/// <summary>
/// Process-wide Windows Forms setup shared by the UI tests.
/// </summary>
internal static class WinFormsTestSupport
{
    private static readonly object Gate = new();
    private static bool _initialized;
    private static bool _available;
    private static Exception? _failure;

    /// <summary>
    /// Initializes Windows Forms once per process and reports whether it is usable.
    /// The first call also pumps a throwaway message loop so that one-time toolkit
    /// initialization (assembly load, GDI+/font/theme setup, first window creation)
    /// does not land inside a test's own deadline.
    /// </summary>
    public static bool Initialize()
    {
        lock (Gate)
        {
            if (!_initialized)
            {
                _initialized = true;
                try
                {
                    _available = PdVmDotNetHost.InitializeWindowsFormsApplication();
                    if (_available)
                    {
                        PumpThrowawayMessageLoop();
                    }
                }
                catch (Exception exception)
                {
                    _failure = exception;
                }
            }

            if (_failure is not null)
            {
                throw new InvalidOperationException("Windows Forms initialization failed", _failure);
            }

            return _available;
        }
    }

    /// <summary>
    /// Closes a form on its own UI thread without touching other message loops in the process.
    /// </summary>
    public static bool CloseForm(object form)
    {
        ArgumentNullException.ThrowIfNull(form);
        var close = form.GetType().GetMethod(
            "Close",
            BindingFlags.Public | BindingFlags.Instance,
            binder: null,
            types: Type.EmptyTypes,
            modifiers: null);
        if (close is null)
        {
            return false;
        }

        var beginInvoke = form.GetType().GetMethod(
            "BeginInvoke",
            BindingFlags.Public | BindingFlags.Instance,
            binder: null,
            types: [typeof(Delegate)],
            modifiers: null);
        if (beginInvoke is null)
        {
            return false;
        }

        Action action = () => close.Invoke(form, null);
        try
        {
            _ = beginInvoke.Invoke(form, [action]);
            return true;
        }
        catch (TargetInvocationException exception)
            when (exception.InnerException is InvalidOperationException or ObjectDisposedException)
        {
            // The window handle is already gone; the owning loop has nothing left to close.
            return false;
        }
    }

    private static void PumpThrowawayMessageLoop()
    {
        if (!OperatingSystem.IsWindows())
        {
            return;
        }

        Exception? failure = null;
        var thread = new Thread(() =>
        {
            try
            {
                RunThrowawayMessageLoop();
            }
            catch (Exception exception)
            {
                failure = exception;
            }
        })
        {
            IsBackground = true,
        };
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        if (!thread.Join(TimeSpan.FromSeconds(30)))
        {
            throw new InvalidOperationException("the Windows Forms warm-up message loop did not exit");
        }

        if (failure is not null)
        {
            throw new InvalidOperationException("the Windows Forms warm-up message loop failed", failure);
        }
    }

    private static void RunThrowawayMessageLoop()
    {
        var formType = Type.GetType(
            "System.Windows.Forms.Form, System.Windows.Forms",
            throwOnError: true)!;
        var application = Type.GetType(
            "System.Windows.Forms.Application, System.Windows.Forms",
            throwOnError: true)!;
        var form = Activator.CreateInstance(formType)!;
        formType.GetProperty("ShowInTaskbar")?.SetValue(form, false);
        using var timer = new System.Threading.Timer(
            _ => CloseForm(form),
            null,
            TimeSpan.FromMilliseconds(100),
            Timeout.InfiniteTimeSpan);
        var run = application.GetMethod(
            "Run",
            BindingFlags.Public | BindingFlags.Static,
            binder: null,
            types: [formType],
            modifiers: null)!;
        _ = run.Invoke(null, [form]);
        (form as IDisposable)?.Dispose();
    }
}
