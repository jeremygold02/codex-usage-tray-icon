#if WPF_UI
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Security.Cryptography;
using System.Text;

namespace CodexUsageTray
{
    internal sealed class WpfUpdatePreparation
    {
        public string ScriptPath { get; internal set; }
        public string StageDirectory { get; internal set; }

        public void Cleanup()
        {
            WpfUpdatePackage.CleanupStaging(StageDirectory);
        }
    }

    internal static class WpfUpdatePackage
    {
        public const string AssetName = "CodexUsageTray-Wpf.zip";
        private const long MaximumFileBytes = 32L * 1024L * 1024L;
        private const long MaximumPackageBytes = 128L * 1024L * 1024L;
        private static readonly string[] PackageFiles = new string[]
        {
            "CodexUsageTray.exe", "CodexUsageTray.exe.config", "Wpf.Ui.dll",
            "Wpf.Ui.Abstractions.dll", "System.Memory.dll", "System.Buffers.dll",
            "System.Numerics.Vectors.dll", "System.Runtime.CompilerServices.Unsafe.dll"
        };

        public static WpfUpdatePreparation Prepare(string archivePath, string appDir, string targetExe,
            string tempDir, string updateId, int pid)
        {
            return PrepareCore(archivePath, appDir, targetExe, tempDir, updateId, pid, "CodexUsageTray.SingleInstance");
        }

#if WPF_UPDATER_TESTS
        internal static WpfUpdatePreparation PrepareForTesting(string archivePath, string appDir, string targetExe,
            string tempDir, string updateId, int pid, string mutexName)
        {
            if (string.IsNullOrEmpty(mutexName) || !mutexName.StartsWith("CodexUsageTray.Test.", StringComparison.Ordinal))
                throw new ArgumentException("The test mutex must use the isolated test prefix.", "mutexName");
            return PrepareCore(archivePath, appDir, targetExe, tempDir, updateId, pid, mutexName);
        }
#endif

        private static WpfUpdatePreparation PrepareCore(string archivePath, string appDir, string targetExe,
            string tempDir, string updateId, int pid, string mutexName)
        {
            if (!IsUpdateId(updateId) || pid <= 0)
                throw new UpdateException("The update staging identifier is invalid.");
            string appPath = Path.GetFullPath(appDir);
            string targetPath = Path.GetFullPath(targetExe);
            if (!string.Equals(Path.GetDirectoryName(targetPath), appPath, StringComparison.OrdinalIgnoreCase) ||
                !string.Equals(Path.GetExtension(targetPath), ".exe", StringComparison.OrdinalIgnoreCase))
                throw new UpdateException("The app executable is not in the expected installation folder.");
            EnsureDirectoryPathSafe(appPath);
            EnsureDirectoryPathSafe(Path.GetFullPath(tempDir));
            EnsureSafeFile(targetPath, true);
            string stage = Path.Combine(appPath, ".codex-wpf-update-" + updateId);
            string incoming = Path.Combine(stage, "incoming");
            string backup = Path.Combine(stage, "backup");
            string script = Path.Combine(tempDir, "update.ps1");
            if (Directory.Exists(stage) || File.Exists(stage) || File.Exists(script))
                throw new UpdateException("The update staging location already exists.");
            try
            {
                Directory.CreateDirectory(stage);
                EnsureDirectoryPathSafe(stage);
                Directory.CreateDirectory(incoming);
                Directory.CreateDirectory(backup);
                EnsureDirectoryPathSafe(incoming);
                EnsureDirectoryPathSafe(backup);

                var expected = new HashSet<string>(PackageFiles, StringComparer.OrdinalIgnoreCase);
                var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                var hashes = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
                long total = 0;
                using (FileStream input = new FileStream(archivePath, FileMode.Open, FileAccess.Read, FileShare.Read))
                using (ZipArchive zip = new ZipArchive(input, ZipArchiveMode.Read, false))
                {
                    foreach (ZipArchiveEntry entry in zip.Entries)
                    {
                        string name = entry.FullName;
                        if (string.IsNullOrEmpty(name) || name.IndexOfAny(new char[] { '/', '\\', ':' }) >= 0 ||
                            !expected.Contains(name) || !seen.Add(name) ||
                            (entry.ExternalAttributes & (int)FileAttributes.ReparsePoint) != 0 ||
                            ((entry.ExternalAttributes >> 16) & 0xF000) == 0xA000)
                            throw new UpdateException("The update package contains an unexpected or unsafe file.");
                        if (entry.Length <= 0 || entry.Length > MaximumFileBytes || entry.Length > MaximumPackageBytes - total)
                            throw new UpdateException("The update package contains an invalid file size.");
                        total += entry.Length;
                        string canonicalName = CanonicalName(name);
                        string output = Path.Combine(incoming, canonicalName);
                        long copied = 0;
                        using (Stream source = entry.Open())
                        using (FileStream destination = new FileStream(output, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                        {
                            byte[] buffer = new byte[81920];
                            int count;
                            while ((count = source.Read(buffer, 0, buffer.Length)) > 0)
                            {
                                copied += count;
                                if (copied > entry.Length || copied > MaximumFileBytes)
                                    throw new UpdateException("An update package file exceeded its declared size.");
                                destination.Write(buffer, 0, count);
                            }
                            if (copied != entry.Length) throw new UpdateException("An update package file is incomplete.");
                            destination.Flush();
                        }
                        if (canonicalName.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ||
                            canonicalName.EndsWith(".dll", StringComparison.OrdinalIgnoreCase))
                            VerifyExecutable(output);
                        hashes.Add(canonicalName, HashFile(output));
                    }
                }
                if (seen.Count != PackageFiles.Length)
                    throw new UpdateException("The WPF update package is missing required files.");
                foreach (string name in PackageFiles)
                {
                    string destination = Path.Combine(appPath, DestinationName(name, targetPath));
                    EnsureSafeFile(destination, false);
                }
                WriteScript(script, appPath, targetPath, stage, tempDir, pid, hashes, mutexName);
                return new WpfUpdatePreparation { ScriptPath = script, StageDirectory = stage };
            }
            catch
            {
                CleanupStaging(stage);
                try { if (File.Exists(script)) File.Delete(script); } catch { }
                throw;
            }
        }

        private static bool IsUpdateId(string id)
        {
            if (id == null || id.Length != 32) return false;
            foreach (char value in id)
                if (!Uri.IsHexDigit(value)) return false;
            return true;
        }

        private static string CanonicalName(string name)
        {
            foreach (string expected in PackageFiles)
                if (string.Equals(expected, name, StringComparison.OrdinalIgnoreCase)) return expected;
            throw new UpdateException("The update package contains an unexpected file.");
        }

        private static string DestinationName(string name, string targetExe)
        {
            if (string.Equals(name, "CodexUsageTray.exe", StringComparison.OrdinalIgnoreCase))
                return Path.GetFileName(targetExe);
            if (string.Equals(name, "CodexUsageTray.exe.config", StringComparison.OrdinalIgnoreCase))
                return Path.GetFileName(targetExe) + ".config";
            return name;
        }

        private static void EnsureDirectoryPathSafe(string path)
        {
            DirectoryInfo current = new DirectoryInfo(path);
            while (current != null)
            {
                if (!current.Exists || (current.Attributes & FileAttributes.ReparsePoint) != 0)
                    throw new UpdateException("An update directory is missing or uses a link. Automatic installation was stopped.");
                current = current.Parent;
            }
        }

        private static void EnsureSafeFile(string path, bool required)
        {
            if (Directory.Exists(path)) throw new UpdateException("An update destination is a directory.");
            if (!File.Exists(path))
            {
                if (required) throw new UpdateException("The installed executable is missing.");
                return;
            }
            FileAttributes attributes = File.GetAttributes(path);
            if ((attributes & (FileAttributes.ReparsePoint | FileAttributes.ReadOnly)) != 0)
                throw new UpdateException("An update destination is a link or read-only file.");
        }

        private static void VerifyExecutable(string path)
        {
            using (var file = File.OpenRead(path))
            using (var reader = new BinaryReader(file))
            {
                if (file.Length < 64 || reader.ReadByte() != 'M' || reader.ReadByte() != 'Z')
                    throw new UpdateException("An update package executable is invalid.");
                file.Position = 0x3c;
                uint peOffset = reader.ReadUInt32();
                if (peOffset < 64 || peOffset > file.Length - 4)
                    throw new UpdateException("An update package executable has an invalid PE header.");
                file.Position = peOffset;
                if (reader.ReadUInt32() != 0x00004550)
                    throw new UpdateException("An update package executable has an invalid PE signature.");
            }
        }

        private static string HashFile(string path)
        {
            using (var algorithm = SHA256.Create())
            using (var file = File.OpenRead(path))
                return BitConverter.ToString(algorithm.ComputeHash(file)).Replace("-", "").ToLowerInvariant();
        }

        internal static void CleanupStaging(string stage)
        {
            try
            {
                if (string.IsNullOrEmpty(stage)) return;
                stage = Path.GetFullPath(stage);
                const string prefix = ".codex-wpf-update-";
                string leaf = Path.GetFileName(stage);
                if (!leaf.StartsWith(prefix, StringComparison.Ordinal) || !IsUpdateId(leaf.Substring(prefix.Length))) return;
                EnsureDirectoryPathSafe(Path.GetDirectoryName(stage));
                if (!Directory.Exists(stage) ||
                    (File.GetAttributes(stage) & FileAttributes.ReparsePoint) != 0) return;
                foreach (string folderName in new string[] { "incoming", "backup" })
                {
                    string folder = Path.Combine(stage, folderName);
                    if (!Directory.Exists(folder) || (File.GetAttributes(folder) & FileAttributes.ReparsePoint) != 0) continue;
                    foreach (string name in PackageFiles)
                    {
                        string path = Path.Combine(folder, name);
                        if (File.Exists(path)) File.Delete(path);
                        string restore = path + ".restore";
                        if (File.Exists(restore)) File.Delete(restore);
                        string replaced = path + ".replaced";
                        if (File.Exists(replaced)) File.Delete(replaced);
                        string failed = path + ".failed";
                        if (File.Exists(failed)) File.Delete(failed);
                    }
                    Directory.Delete(folder, false);
                }
                Directory.Delete(stage, false);
            }
            catch { /* Keep an unremovable staging directory for inspection. */ }
        }

        private static string Ps(string value) { return "'" + (value ?? "").Replace("'", "''") + "'"; }

        private static void WriteScript(string script, string appDir, string targetExe, string stage,
            string tempDir, int pid, Dictionary<string, string> hashes, string mutexName)
        {
            var lines = new List<string>();
            lines.Add("$ErrorActionPreference = 'Stop'");
            lines.Add("$appDir = " + Ps(appDir));
            lines.Add("$target = " + Ps(targetExe));
            lines.Add("$stage = " + Ps(stage));
            lines.Add("$incoming = Join-Path $stage 'incoming'");
            lines.Add("$backup = Join-Path $stage 'backup'");
            lines.Add("$tempDir = " + Ps(tempDir));
            lines.Add("$log = " + Ps(Path.ChangeExtension(script, ".log")));
            lines.Add("$pidToWait = " + pid.ToString(CultureInfo.InvariantCulture));
            lines.Add("$mutexName = " + Ps(mutexName));
            lines.Add("$items = @(");
            for (int index = 0; index < PackageFiles.Length; index++)
            {
                string name = PackageFiles[index];
                lines.Add("  @{ Source = " + Ps(name) + "; Destination = " + Ps(DestinationName(name, targetExe)) +
                    "; Hash = " + Ps(hashes[name]) + "; Existed = $false; Installed = $false }" +
                    (index + 1 < PackageFiles.Length ? "," : ""));
            }
            lines.Add(")");
            lines.AddRange(new string[]
            {
                "function Log([string]$message) { try { Add-Content -LiteralPath $log -Value \"$(Get-Date -Format o) $message\" } catch {} }",
                UpdateService.FailureNotificationScript(),
                "function Assert-PlainDirectory([string]$path) {",
                "  $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop",
                "  while ($null -ne $item) {",
                "    if (-not ($item -is [IO.DirectoryInfo]) -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'An update folder is missing or is a link.' }; $item = $item.Parent",
                "  }",
                "}",
                "function Assert-PlainFile([string]$path, [bool]$required) {",
                "  $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue",
                "  if ($null -eq $item) { if ($required) { throw 'A required update file is missing.' }; return }",
                "  if ($item.PSIsContainer -or ($item.Attributes -band ([IO.FileAttributes]::ReparsePoint -bor [IO.FileAttributes]::ReadOnly))) { throw 'An update file is a link, directory, or read-only.' }",
                "}",
                "function Get-FileSha256([string]$path) {",
                "  $algorithm = [Security.Cryptography.SHA256]::Create(); $stream = $null",
                "  try { $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read); return [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '') }",
                "  finally { if ($null -ne $stream) { $stream.Dispose() }; $algorithm.Dispose() }",
                "}",
                "function Get-AppProcesses { @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object { $_.ExecutablePath -and [string]::Equals($_.ExecutablePath, $target, [StringComparison]::OrdinalIgnoreCase) }) }",
                "function Remove-Staging {",
                "  Assert-PlainDirectory $stage; Assert-PlainDirectory $incoming; Assert-PlainDirectory $backup",
                "  foreach ($item in $items) {",
                "    foreach ($folder in @($incoming, $backup)) { foreach ($suffix in @('', '.restore', '.replaced', '.failed')) { $file = (Join-Path $folder $item.Source) + $suffix; if (Test-Path -LiteralPath $file -PathType Leaf) { Remove-Item -LiteralPath $file -Force } } }",
                "  }",
                "  [IO.Directory]::Delete($incoming, $false); [IO.Directory]::Delete($backup, $false); [IO.Directory]::Delete($stage, $false)",
                "}",
                "$success = $false; $rollbackComplete = $true; $exitCode = 1; $mutex = $null; $ownsMutex = $false; $started = $null",
                "try {",
                "  Assert-PlainDirectory $appDir; Assert-PlainDirectory $stage; Assert-PlainDirectory $incoming; Assert-PlainDirectory $backup; Assert-PlainFile $target $true",
                "  $deadline = (Get-Date).AddSeconds(20)",
                "  while ((Get-Process -Id $pidToWait -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }",
                "  if ((Get-Process -Id $pidToWait -ErrorAction SilentlyContinue) -or @(Get-AppProcesses).Count -gt 0) { throw 'The current app is still running.' }",
                "  $mutex = New-Object System.Threading.Mutex($false, $mutexName)",
                "  try { $ownsMutex = $mutex.WaitOne(0) } catch [System.Threading.AbandonedMutexException] { $ownsMutex = $true }",
                "  if (-not $ownsMutex) { throw 'Another app instance started during the update.' }",
                "  foreach ($item in $items) {",
                "    $source = Join-Path $incoming $item.Source; $destination = Join-Path $appDir $item.Destination; $saved = Join-Path $backup $item.Source",
                "    Assert-PlainFile $source $true; Assert-PlainFile $destination $false",
                "    if (-not [string]::Equals((Get-FileSha256 $source), $item.Hash, [StringComparison]::OrdinalIgnoreCase)) { throw 'A staged update file failed verification.' }",
                "    $item.Existed = Test-Path -LiteralPath $destination -PathType Leaf",
                "    if ($item.Existed) { if (Test-Path -LiteralPath $saved) { throw 'An update backup already exists.' }; [IO.File]::Copy($destination, $saved, $false) }",
                "  }",
                "  foreach ($item in $items) {",
                "    $source = Join-Path $incoming $item.Source; $destination = Join-Path $appDir $item.Destination",
                "    Assert-PlainFile $source $true; Assert-PlainFile $destination $false",
                "    if ($item.Existed) { $replaced = Join-Path $backup ($item.Source + '.replaced'); if (Test-Path -LiteralPath $replaced) { throw 'A replacement backup already exists.' }; [IO.File]::Replace($source, $destination, $replaced, $true) } else { [IO.File]::Move($source, $destination) }",
                "    $item.Installed = $true",
                "  }",
                "  $mutex.ReleaseMutex(); $ownsMutex = $false; $mutex.Dispose(); $mutex = $null",
                "  $started = Start-Process -FilePath $target -WorkingDirectory $appDir -WindowStyle Hidden -PassThru",
                "  if ($null -eq $started -or $started.WaitForExit(5000)) { throw 'The updated app exited during startup.' }",
                "  $success = $true; $exitCode = 0; Log 'Update installed and app started.'",
                "} catch {",
                "  Log ('Update failed: ' + $_.Exception.Message)",
                "  if ($null -ne $started) { try { $started.Refresh(); if (-not $started.HasExited) { Stop-Process -Id $started.Id -Force -ErrorAction Stop; if (-not $started.WaitForExit(5000)) { throw 'Updated process did not stop.' } } } catch { $rollbackComplete = $false; Log ('Could not stop updated app: ' + $_.Exception.Message) } }",
                "  $anyInstalled = @($items | Where-Object { $_.Installed }).Count -gt 0",
                "  if ($rollbackComplete -and $anyInstalled -and -not $ownsMutex) {",
                "    try { if ($null -eq $mutex) { $mutex = New-Object System.Threading.Mutex($false, $mutexName) }; try { $ownsMutex = $mutex.WaitOne(5000) } catch [System.Threading.AbandonedMutexException] { $ownsMutex = $true }; if (-not $ownsMutex) { throw 'Another app instance is running.' } } catch { $rollbackComplete = $false; Log ('Could not secure rollback: ' + $_.Exception.Message) }",
                "  }",
                "  $canRollback = $rollbackComplete",
                "  for ($i = $items.Count - 1; $canRollback -and $i -ge 0; $i--) {",
                "    $item = $items[$i]; if (-not $item.Installed) { continue }",
                "    try {",
                "      Assert-PlainDirectory $appDir; Assert-PlainDirectory $backup",
                "      $destination = Join-Path $appDir $item.Destination; $saved = Join-Path $backup $item.Source; Assert-PlainFile $destination $false",
                "      if ($item.Existed) {",
                "        Assert-PlainFile $saved $true; $restore = Join-Path $incoming ($item.Source + '.restore')",
                "        if (Test-Path -LiteralPath $restore) { throw 'Rollback staging file already exists.' }",
                "        [IO.File]::Copy($saved, $restore, $false)",
                "        if (Test-Path -LiteralPath $destination -PathType Leaf) { $failed = Join-Path $incoming ($item.Source + '.failed'); if (Test-Path -LiteralPath $failed) { throw 'A failed-file staging path already exists.' }; [IO.File]::Replace($restore, $destination, $failed, $true) } else { [IO.File]::Move($restore, $destination) }",
                "      } elseif (Test-Path -LiteralPath $destination -PathType Leaf) { [IO.File]::Delete($destination) }",
                "    } catch { $rollbackComplete = $false; Log ('Rollback failed for ' + $item.Destination + ': ' + $_.Exception.Message) }",
                "  }",
                "  if ($ownsMutex) { $mutex.ReleaseMutex(); $ownsMutex = $false }; if ($null -ne $mutex) { $mutex.Dispose(); $mutex = $null }",
                "  if ($rollbackComplete -and @(Get-AppProcesses).Count -eq 0) {",
                "    try { Assert-PlainFile $target $true; $restored = Start-Process -FilePath $target -WorkingDirectory $appDir -WindowStyle Hidden -ArgumentList '--update-failed' -PassThru; if ($null -eq $restored -or $restored.WaitForExit(5000)) { throw 'The previous app exited during restart.' } } catch { $rollbackComplete = $false; Log ('Previous app restart failed: ' + $_.Exception.Message) }",
                "  }",
                "  if ($rollbackComplete) { Show-UpdateFailure 'Update could not be installed. The previous app remains available.' }",
                "  else { Show-UpdateFailure 'Update failed and the previous app could not be restored. Manual installation may be required.' }",
                "} finally {",
                "  if ($ownsMutex) { $mutex.ReleaseMutex() }; if ($null -ne $mutex) { $mutex.Dispose() }",
                "  if ($success -or $rollbackComplete) { try { Remove-Staging } catch { Log ('Staging cleanup failed: ' + $_.Exception.Message) } }",
                "  if ($success) { Remove-Item -LiteralPath $log -Force -ErrorAction SilentlyContinue; Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue; try { [IO.Directory]::Delete($tempDir, $false) } catch {} }",
                "}",
                "exit $exitCode"
            });
            using (var output = new FileStream(script, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            using (var writer = new StreamWriter(output, new UTF8Encoding(false)))
                writer.Write(string.Join(Environment.NewLine, lines.ToArray()));
        }
    }
}
#endif
