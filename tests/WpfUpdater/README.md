# WPF updater checks

From the repository root, build and run:

```powershell
dotnet build tests/WpfUpdater/WpfUpdater.Tests.csproj -c Release --warnaserror
& .\tests\WpfUpdater\bin\Release\net481\WpfUpdater.Tests.exe
```

The tests create a unique temporary install, use synthetic executables and a test-only mutex, and run the generated updater with hidden processes. They never use the live tray's executable, mutex, settings, history, or network. They validate asset selection and integrity, reject unsafe archives, inspect generated PowerShell syntax, and exercise successful installation, running-process handoff, startup-failure rollback, and locked destinations at every replacement position. A failed run reports the temporary directory for inspection.
