using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Threading;

namespace CodexUsageTray
{
    internal static class WpfUpdaterTests
    {
        private static readonly string[] Names = {
            "CodexUsageTray.exe", "CodexUsageTray.exe.config", "Wpf.Ui.dll", "Wpf.Ui.Abstractions.dll",
            "System.Memory.dll", "System.Buffers.dll", "System.Numerics.Vectors.dll", "System.Runtime.CompilerServices.Unsafe.dll"
        };
        private static string root;
        private static byte[] fakeApp;
        private static int assertions;
        private static string lastProcessDiagnostics;

        private static int Main()
        {
            if (Path.GetFileNameWithoutExtension(Process.GetCurrentProcess().MainModule.FileName)
                .StartsWith("wpf-test-app-", StringComparison.OrdinalIgnoreCase))
                return RunFakeApp();

            root = Path.Combine(Path.GetTempPath(), "codex-wpf-updater-tests-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(root);
            File.WriteAllText(Path.Combine(root, ".test-root"), "Synthetic updater tests only");
            try
            {
                fakeApp = File.ReadAllBytes(Process.GetCurrentProcess().MainModule.FileName);
                TestReleaseAssetAndIntegrity();
                TestPreparation();
                TestUnsafeArchives();
                TestInstallAndRollback();
                Console.WriteLine("WPF updater tests passed (" + assertions + " assertions).");
                DeleteTestRoot();
                return 0;
            }
            catch (Exception error)
            {
                Console.Error.WriteLine(error);
                PrintFailureDiagnostics();
                Console.Error.WriteLine("Synthetic test files retained: " + root);
                return 1;
            }
        }

        private static int RunFakeApp()
        {
            string directory = AppDomain.CurrentDomain.BaseDirectory;
            try
            {
                string mutexName = File.ReadAllText(Path.Combine(directory, "synthetic-mutex.txt"));
                bool createdNew;
                using (var mutex = new Mutex(true, mutexName, out createdNew))
                {
                    if (!createdNew)
                    {
                        File.WriteAllText(Path.Combine(directory, "synthetic-error.txt"), "The synthetic app's isolated mutex already exists.");
                        return 92;
                    }
                    File.WriteAllText(Path.Combine(directory, "synthetic-process-id.txt"), Process.GetCurrentProcess().Id.ToString());
                    string exitSignal = Path.Combine(directory, "synthetic-exit.txt");
                    for (int tick = 0; tick < 300; tick++)
                    {
                        if (File.Exists(exitSignal))
                        {
                            File.Delete(exitSignal);
                            File.Delete(Path.Combine(directory, "synthetic-process-id.txt"));
                            return 0;
                        }
                        Thread.Sleep(100);
                    }
                    return 0;
                }
            }
            catch (Exception error)
            {
                try { File.WriteAllText(Path.Combine(directory, "synthetic-error.txt"), error.ToString()); } catch { }
                Console.Error.WriteLine(error);
                return 91;
            }
        }

        private static void TestReleaseAssetAndIntegrity()
        {
            Check(ExpectedAsset("CodexUsageTray-Wpf.zip"), "WPF bundle is accepted");
            Check(ExpectedAsset("codexusagetray-wpf.ZIP"), "WPF bundle comparison ignores case");
            Check(!ExpectedAsset("CodexUsageTray.exe") && !ExpectedAsset("other/CodexUsageTray-Wpf.zip") &&
                !ExpectedAsset("CodexUsageTray-Wpf.zip.exe"), "legacy and disguised assets are rejected");
            var legacy = new Dictionary<string, object> { { "name", "CodexUsageTray.exe" } };
            var wpf = new Dictionary<string, object> { { "name", "CodexUsageTray-Wpf.zip" } };
            var release = new Dictionary<string, object> { { "assets", new object[] { legacy, wpf } } };
            Check(ReferenceEquals(Invoke("ReleaseAsset", release), wpf), "release picks WPF package over legacy executable");
            release["assets"] = new object[] { legacy };
            Check(Invoke("ReleaseAsset", release) == null, "old releases without WPF package are not installable");

            object[] digestArguments = { "sha256:" + new string('a', 64), null };
            Check((bool)InvokeWithArgs("TryParseSha256Digest", digestArguments) &&
                (string)digestArguments[1] == new string('a', 64), "valid SHA-256 metadata is accepted");
            foreach (string invalid in new[] { "", "sha256:abcd", "md5:" + new string('a', 64), "sha256:" + new string('z', 64) })
            {
                object[] args = { invalid, null };
                Check(!(bool)InvokeWithArgs("TryParseSha256Digest", args), "invalid digest rejected: " + invalid);
            }
            string assetFile = Path.Combine(root, "integrity.bin");
            File.WriteAllBytes(assetFile, new byte[] { 1, 2, 3 });
            string hash;
            using (SHA256 sha = SHA256.Create()) hash = BitConverter.ToString(sha.ComputeHash(File.ReadAllBytes(assetFile))).Replace("-", "").ToLowerInvariant();
            Invoke("VerifyAssetFile", assetFile, 3L, hash);
            ExpectFailure(() => Invoke("VerifyAssetFile", assetFile, 3L, new string('0', 64)), "digest mismatch");
            ExpectFailure(() => Invoke("VerifyAssetFile", assetFile, 4L, hash), "asset size mismatch");
            Check(AppVersion.Current == AppVersion.DisplayVersion(BuildVersion.Version), "baked release version is respected");
            Check(RunPowerShellCommand("[Console]::Error.WriteLine('synthetic stderr'); exit 41") == 41 &&
                lastProcessDiagnostics.Contains("synthetic stderr"), "child process stderr is captured for failures");
        }

        private static bool ExpectedAsset(string name) { return (bool)Invoke("IsExpectedReleaseAsset", name); }

        private static void TestPreparation()
        {
            var env = NewEnvironment("valid");
            string archive = CreateArchive(env, ValidEntries(fakeApp));
            var preparation = WpfUpdatePackage.Prepare(archive, env.App, env.Target, env.Temp,
                Guid.NewGuid().ToString("N"), int.MaxValue);
            Check(Directory.Exists(preparation.StageDirectory) && File.Exists(preparation.ScriptPath), "verified archive is staged and script created");
            Check(File.ReadAllText(preparation.ScriptPath).Contains("CodexUsageTray.SingleInstance"), "production updater uses the app mutex");
            foreach (string name in Names) Check(File.Exists(Path.Combine(preparation.StageDirectory, "incoming", name)), "staged " + name);
            Check(File.ReadAllText(Path.Combine(env.App, "user-note.txt")) == "keep", "preparing preserves unrelated files");
            CheckScriptSyntax(preparation.ScriptPath);
            preparation.Cleanup();
            Check(!Directory.Exists(preparation.StageDirectory), "cleanup removes isolated staging files");

            string unrelated = Path.Combine(env.App, "unrelated-folder");
            Directory.CreateDirectory(Path.Combine(unrelated, "incoming"));
            string preserved = Path.Combine(unrelated, "incoming", Names[0]);
            File.WriteAllText(preserved, "unrelated");
            WpfUpdatePackage.CleanupStaging(unrelated);
            Check(File.Exists(preserved) && File.ReadAllText(preserved) == "unrelated", "cleanup rejects unrelated directories");
        }

        private static void TestUnsafeArchives()
        {
            foreach (string name in new[] { "../evil.dll", "/evil.dll", "C:/evil.dll", "nested/evil.dll", "nested\\evil.dll", "file:stream", "folder/", "extra.dll" })
            {
                var env = NewEnvironment("unsafe");
                var entries = ValidEntries(fakeApp);
                entries.Add(new KeyValuePair<string, byte[]>(name, new byte[] { 77, 90, 1 }));
                ExpectPrepareFailure(env, CreateArchive(env, entries), "unsafe or unexpected entry: " + name);
            }
            {
                var env = NewEnvironment("duplicate");
                var entries = ValidEntries(fakeApp);
                entries.Add(new KeyValuePair<string, byte[]>("WPF.UI.DLL", new byte[] { 77, 90, 1 }));
                ExpectPrepareFailure(env, CreateArchive(env, entries), "case-insensitive duplicate");
            }
            {
                var env = NewEnvironment("missing");
                var entries = ValidEntries(fakeApp);
                entries.RemoveAt(entries.Count - 1);
                ExpectPrepareFailure(env, CreateArchive(env, entries), "missing dependency");
            }
            {
                var env = NewEnvironment("invalid-pe");
                var entries = ValidEntries(fakeApp);
                entries[2] = new KeyValuePair<string, byte[]>(Names[2], Encoding.ASCII.GetBytes("not a PE"));
                ExpectPrepareFailure(env, CreateArchive(env, entries), "invalid DLL header");
            }
            {
                var env = NewEnvironment("empty");
                var entries = ValidEntries(fakeApp);
                entries[1] = new KeyValuePair<string, byte[]>(Names[1], new byte[0]);
                ExpectPrepareFailure(env, CreateArchive(env, entries), "empty required file");
            }
            {
                var env = NewEnvironment("malformed");
                string archive = Path.Combine(env.Case, "archive.zip");
                File.WriteAllBytes(archive, Encoding.ASCII.GetBytes("not a zip"));
                ExpectPrepareFailure(env, archive, "malformed archive");
            }
            {
                var env = NewEnvironment("oversize");
                string archive = CreateArchive(env, ValidEntries(fakeApp));
                byte[] bytes = File.ReadAllBytes(archive);
                int central = FindCentralDirectory(bytes);
                // Forged ZIP metadata claims the first item exceeds the per-file limit.
                byte[] tooLarge = BitConverter.GetBytes(33 * 1024 * 1024);
                Array.Copy(tooLarge, 0, bytes, central + 24, 4);
                File.WriteAllBytes(archive, bytes);
                ExpectPrepareFailure(env, archive, "oversized archive entry");
            }
        }

        private static void TestInstallAndRollback()
        {
            // Each updater script operates only in a newly created synthetic app folder.
            // Use a unique test mutex, so a running real tray can never interfere.
            RunInstallCase("success", fakeApp, false);
            string failingExe = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "where.exe");
            Check(File.Exists(failingExe), "Windows synthetic failing executable is available");
            RunInstallCase("startup-failure", File.ReadAllBytes(failingExe), true);
            TestRunningProcessHandoff();
            for (int position = 0; position < Names.Length; position++) RunLockedCase(position);
        }

        private static void TestRunningProcessHandoff()
        {
            var env = NewEnvironment("handoff");
            string archive = CreateArchive(env, ValidEntries(fakeApp));
            Process old = null;
            WpfUpdatePreparation preparation = null;
            try
            {
                old = Process.Start(new ProcessStartInfo {
                    FileName = env.Target, WorkingDirectory = env.App,
                    UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden
                });
                Check(old != null, "synthetic previous app starts");
                WaitForFakeApp(env);
                int previousPid = old.Id;
                preparation = WpfUpdatePackage.PrepareForTesting(archive, env.App, env.Target, env.Temp,
                    Guid.NewGuid().ToString("N"), previousPid, env.MutexName);
                ThreadPool.QueueUserWorkItem(delegate {
                    Thread.Sleep(1200);
                    File.WriteAllText(Path.Combine(env.App, "synthetic-exit.txt"), "close");
                });
                Check(RunPowerShell(preparation.ScriptPath) == 0, "updater waits for previous process and installs");
                Check(old.WaitForExit(5000) && old.ExitCode == 0, "previous process exits normally");
                WaitForFakeApp(env);
                int nextPid;
                Check(int.TryParse(File.ReadAllText(Path.Combine(env.App, "synthetic-process-id.txt")), out nextPid) &&
                    nextPid != previousPid, "updated process starts with a new PID");
            }
            finally
            {
                StopFakeApp(env);
                if (old != null)
                {
                    try { if (!old.HasExited && string.Equals(old.MainModule.FileName, env.Target, StringComparison.OrdinalIgnoreCase)) { old.Kill(); old.WaitForExit(5000); } }
                    catch (InvalidOperationException) { }
                    old.Dispose();
                }
                if (preparation != null) preparation.Cleanup();
            }
        }

        private static void RunLockedCase(int position)
        {
            var env = NewEnvironment("locked-" + position);
            var oldHashes = new Dictionary<string, string>();
            foreach (string name in Names)
            {
                string destination = Destination(env, name);
                if (name == Names[1]) File.WriteAllText(destination, "<configuration><!-- old --></configuration>");
                else if (name != Names[0]) File.WriteAllText(destination, "original " + name);
                oldHashes.Add(destination, Hash(destination));
            }
            string archive = CreateArchive(env, ValidEntries(fakeApp));
            var preparation = Prepare(env, archive);
            try
            {
                // Readers may copy the old file for backup; lack of delete sharing
                // makes File.Replace fail at the selected transaction position.
                using (new FileStream(Destination(env, Names[position]), FileMode.Open, FileAccess.Read, FileShare.Read))
                {
                    Check(RunPowerShell(preparation.ScriptPath) == 1, "locked file rejects replacement " + position);
                }
                foreach (var old in oldHashes)
                    Check(Hash(old.Key) == old.Value, "locked file rollback restores " + Path.GetFileName(old.Key) + " at " + position);
                Check(File.ReadAllText(Path.Combine(env.App, "user-note.txt")) == "keep", "locked update preserves unrelated file " + position);
                WaitForFakeApp(env);
            }
            finally { StopFakeApp(env); preparation.Cleanup(); }
        }

        private static string Destination(TestEnvironment env, string name)
        {
            if (name == Names[0]) return env.Target;
            if (name == Names[1]) return env.Target + ".config";
            return Path.Combine(env.App, name);
        }

        private static void RunInstallCase(string name, byte[] newExe, bool shouldRollback)
        {
            var env = NewEnvironment(name);
            string oldHash = Hash(env.Target);
            // Existing and new managed files plus an unrelated file exercise both rollback modes.
            File.WriteAllText(Path.Combine(env.App, "Wpf.Ui.dll"), "old-library");
            string archive = CreateArchive(env, ValidEntries(newExe));
            var preparation = Prepare(env, archive);
            try
            {
                string script = File.ReadAllText(preparation.ScriptPath);
                Check(script.Contains("CodexUsageTray.Test."), "synthetic updater uses an isolated mutex");
                int exitCode = RunPowerShell(preparation.ScriptPath);
                Check(exitCode == (shouldRollback ? 1 : 0), name + " updater exit status: " + exitCode);
                Check(File.ReadAllText(Path.Combine(env.App, "user-note.txt")) == "keep", name + " preserves unrelated file");
                if (shouldRollback)
                {
                    Check(Hash(env.Target) == oldHash, "startup failure restores original executable");
                    Check(File.ReadAllText(Path.Combine(env.App, "Wpf.Ui.dll")) == "old-library", "startup failure restores original library");
                    Check(!File.Exists(Path.Combine(env.App, "System.Memory.dll")), "rollback removes newly added libraries");
                }
                else
                {
                    Check(Hash(env.Target) == HashBytes(newExe), "updated executable installed under renamed target");
                    Check(File.ReadAllBytes(Path.Combine(env.App, "Wpf.Ui.dll"))[0] == 77, "updated library installed");
                    Check(File.Exists(env.Target + ".config"), "renamed executable configuration installed");
                }
                WaitForFakeApp(env);
            }
            finally { StopFakeApp(env); preparation.Cleanup(); }
        }

        private sealed class TestEnvironment { public string Case, App, Temp, Target, MutexName; }

        private static TestEnvironment NewEnvironment(string label)
        {
            string folder = Path.Combine(root, label + "-" + Guid.NewGuid().ToString("N"));
            var env = new TestEnvironment { Case = folder, App = Path.Combine(folder, "app"), Temp = Path.Combine(folder, "temp") };
            Directory.CreateDirectory(env.App);
            Directory.CreateDirectory(env.Temp);
            env.Target = Path.Combine(env.App, "wpf-test-app-" + Guid.NewGuid().ToString("N") + ".exe");
            env.MutexName = "CodexUsageTray.Test." + Guid.NewGuid().ToString("N");
            File.WriteAllBytes(env.Target, fakeApp);
            File.WriteAllText(Path.Combine(env.App, "user-note.txt"), "keep");
            File.WriteAllText(Path.Combine(env.App, "synthetic-mutex.txt"), env.MutexName);
            return env;
        }

        private static WpfUpdatePreparation Prepare(TestEnvironment env, string archive)
        {
            // This PID does not exist. Never pass the real tray PID to a generated script.
            return WpfUpdatePackage.PrepareForTesting(archive, env.App, env.Target, env.Temp,
                Guid.NewGuid().ToString("N"), int.MaxValue, env.MutexName);
        }

        private static List<KeyValuePair<string, byte[]>> ValidEntries(byte[] executable)
        {
            var entries = new List<KeyValuePair<string, byte[]>>();
            foreach (string name in Names)
            {
                byte[] data = name == Names[0] ? executable : name == Names[1] ? Encoding.UTF8.GetBytes("<configuration/>") : fakeApp;
                entries.Add(new KeyValuePair<string, byte[]>(name, data));
            }
            return entries;
        }

        private static string CreateArchive(TestEnvironment env, List<KeyValuePair<string, byte[]>> entries)
        {
            string path = Path.Combine(env.Case, "archive.zip");
            using (var file = new FileStream(path, FileMode.CreateNew))
            using (var archive = new ZipArchive(file, ZipArchiveMode.Create))
                foreach (var entry in entries)
                    using (Stream output = archive.CreateEntry(entry.Key).Open()) output.Write(entry.Value, 0, entry.Value.Length);
            return path;
        }

        private static void ExpectPrepareFailure(TestEnvironment env, string archive, string description)
        {
            string before = Hash(env.Target);
            ExpectFailure(() => Prepare(env, archive), description);
            Check(Hash(env.Target) == before && File.ReadAllText(Path.Combine(env.App, "user-note.txt")) == "keep", description + " preserves installed files");
            Check(Directory.GetDirectories(env.App, ".codex-wpf-update-*").Length == 0, description + " cleans staging");
        }

        private static int FindCentralDirectory(byte[] bytes)
        {
            for (int i = 0; i < bytes.Length - 28; i++)
                if (bytes[i] == 0x50 && bytes[i + 1] == 0x4b && bytes[i + 2] == 0x01 && bytes[i + 3] == 0x02) return i;
            throw new InvalidOperationException("ZIP central directory not found.");
        }

        private static void CheckScriptSyntax(string script)
        {
            string command = "$t=$null; $e=$null; [void][Management.Automation.Language.Parser]::ParseFile('" + script.Replace("'", "''") + "',[ref]$t,[ref]$e); if($e.Count){exit 1}";
            Check(RunPowerShellCommand(command) == 0, "generated updater PowerShell parses");
        }

        private static int RunPowerShell(string script)
        {
            return RunProcess(new ProcessStartInfo { FileName = PowerShellPath(), Arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"" + script + "\"" });
        }

        private static int RunPowerShellCommand(string command)
        {
            string encoded = Convert.ToBase64String(Encoding.Unicode.GetBytes(command));
            return RunProcess(new ProcessStartInfo { FileName = PowerShellPath(), Arguments = "-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand " + encoded });
        }

        private static string PowerShellPath() { return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), @"WindowsPowerShell\v1.0\powershell.exe"); }

        private static int RunProcess(ProcessStartInfo info)
        {
            info.UseShellExecute = false;
            info.CreateNoWindow = true;
            info.WindowStyle = ProcessWindowStyle.Hidden;
            info.RedirectStandardOutput = true;
            info.RedirectStandardError = true;
            var captured = new StringBuilder();
            using (var child = new Process())
            {
                child.StartInfo = info;
                child.OutputDataReceived += delegate(object sender, DataReceivedEventArgs e) { if (e.Data != null) lock (captured) captured.AppendLine(e.Data); };
                child.ErrorDataReceived += delegate(object sender, DataReceivedEventArgs e) { if (e.Data != null) lock (captured) captured.AppendLine(e.Data); };
                lastProcessDiagnostics = "";
                if (!child.Start()) throw new InvalidOperationException("Synthetic updater process could not start.");
                child.BeginOutputReadLine();
                child.BeginErrorReadLine();
                if (!child.WaitForExit(30000)) throw new TimeoutException("Synthetic updater process did not finish.");
                // Drain asynchronous readers so the logs survive ephemeral CI runners.
                child.WaitForExit();
                lock (captured) lastProcessDiagnostics = captured.ToString();
                return child.ExitCode;
            }
        }

        private static void PrintFailureDiagnostics()
        {
            if (!string.IsNullOrEmpty(lastProcessDiagnostics))
                Console.Error.WriteLine("Last synthetic child output:\n" + lastProcessDiagnostics);
            if (string.IsNullOrEmpty(root) || !Directory.Exists(root)) return;
            try
            {
                var files = Directory.GetFiles(root, "update.log", SearchOption.AllDirectories)
                    .Concat(Directory.GetFiles(root, "synthetic-error.txt", SearchOption.AllDirectories))
                    .OrderByDescending(File.GetLastWriteTimeUtc).Take(8);
                foreach (string file in files)
                    Console.Error.WriteLine("Synthetic diagnostic " + file + ":\n" + File.ReadAllText(file));
            }
            catch (Exception error) { Console.Error.WriteLine("Could not collect synthetic diagnostics: " + error.Message); }
        }

        private static void WaitForFakeApp(TestEnvironment env)
        {
            string marker = Path.Combine(env.App, "synthetic-process-id.txt");
            for (int attempt = 0; attempt < 30 && !File.Exists(marker); attempt++) Thread.Sleep(100);
            Check(File.Exists(marker), "synthetic app restarted");
        }

        private static void StopFakeApp(TestEnvironment env)
        {
            string marker = Path.Combine(env.App, "synthetic-process-id.txt");
            int pid;
            if (!File.Exists(marker) || !int.TryParse(File.ReadAllText(marker), out pid)) return;
            try
            {
                using (Process process = Process.GetProcessById(pid))
                {
                    if (string.Equals(process.MainModule.FileName, env.Target, StringComparison.OrdinalIgnoreCase))
                    { process.Kill(); process.WaitForExit(5000); }
                }
            }
            catch (ArgumentException) { }
            catch (InvalidOperationException) { }
        }

        private static string Hash(string path) { return HashBytes(File.ReadAllBytes(path)); }
        private static string HashBytes(byte[] bytes) { using (SHA256 sha = SHA256.Create()) return BitConverter.ToString(sha.ComputeHash(bytes)); }

        private static object Invoke(string name, params object[] args) { return InvokeWithArgs(name, args); }
        private static object InvokeWithArgs(string name, object[] args)
        {
            try { return typeof(UpdateService).GetMethod(name, BindingFlags.NonPublic | BindingFlags.Static).Invoke(null, args); }
            catch (TargetInvocationException error) { throw error.InnerException; }
        }

        private static void ExpectFailure(Action action, string description)
        {
            try { action(); }
            catch (Exception) { assertions++; return; }
            throw new InvalidOperationException("Expected rejection: " + description);
        }

        private static void Check(bool condition, string description)
        {
            assertions++;
            if (!condition) throw new InvalidOperationException("Failed: " + description);
        }

        private static void DeleteTestRoot()
        {
            string parent = Path.GetFullPath(Path.GetTempPath()).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
            string target = Path.GetFullPath(root);
            if (!target.StartsWith(parent + "codex-wpf-updater-tests-", StringComparison.OrdinalIgnoreCase) ||
                !File.Exists(Path.Combine(target, ".test-root")) || (File.GetAttributes(target) & FileAttributes.ReparsePoint) != 0)
                throw new InvalidOperationException("Refusing to remove an unexpected test directory.");
            Directory.Delete(target, true);
        }
    }
}
