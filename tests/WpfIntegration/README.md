# Private-desktop WPF integration checks

From the repository root, build with `dotnet build tests/WpfIntegration/WpfIntegration.csproj`. Then run `tests/WpfIntegration/bin/Debug/net481/WpfIntegration.exe <new-output-directory>`.

The executable launches its own child on a dedicated, non-visible Windows desktop, verifies that the child is not on the input desktop, and never switches desktops. The child initializes the production WPF resources and constructs the real popup, settings and history forms with synthetic values. A temporary history file and all captures are placed under the selected output directory. No tray context, network refresh, real settings store, credentials, or user history is opened. The settings save callback is replaced by an in-memory delegate.

Captures include the WinForms window and embedded WPF visuals separately, because Windows may omit an embedded WPF surface from a window capture on an invisible desktop. Images ending in `-composite.png` overlay the rendered WPF visual at its host's bounds on the raw window capture, to make the combined layout easier to inspect; raw captures remain available. `result.txt` records the outcome and `error.txt` contains any exception. Use a new output directory for each run; existing result and image files are not overwritten.

To run an already compiled legacy test assembly on a private desktop, invoke `WpfIntegration.exe --legacy <test-exe> <rate-limits-fixture.json> <new-output-directory>`. Its entry point runs in the isolated child process, with temporary directories redirected; the existing test output is saved to `legacy-log.txt`. The launcher itself does not build or replace the tray app.
