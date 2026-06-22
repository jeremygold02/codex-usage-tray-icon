using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Net;
using System.Text;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal sealed class UpdateInfo
    {
        public string CurrentVersion { get; set; }
        public string LatestVersion { get; set; }
        public bool UpdateAvailable { get; set; }
        public bool CanInstall { get; set; }
        public string ReleaseUrl { get; set; }
        public string RepoUrl { get; set; }
        public string AssetName { get; set; }
        public string AssetUrl { get; set; }
        public string Message { get; set; }
    }

    internal sealed class UpdateException : Exception
    {
        public UpdateException(string message)
            : base(message)
        {
        }

        public UpdateException(string message, Exception innerException)
            : base(message, innerException)
        {
        }
    }

    internal static class UpdateService
    {
        private const int GitHubTimeoutMilliseconds = 30000;
        private const int DownloadTimeoutMilliseconds = 60000;

        public static bool CanInstallUpdates()
        {
            if (Environment.OSVersion.Platform != PlatformID.Win32NT)
            {
                return false;
            }

            string executablePath = Application.ExecutablePath;
            return !string.IsNullOrEmpty(executablePath)
                && File.Exists(executablePath)
                && string.Equals(Path.GetExtension(executablePath), ".exe", StringComparison.OrdinalIgnoreCase);
        }

        public static UpdateInfo CheckForUpdate()
        {
            Dictionary<string, object> release = LatestRelease();
            if (release == null)
            {
                return new UpdateInfo
                {
                    CurrentVersion = AppVersion.Current,
                    LatestVersion = "",
                    UpdateAvailable = false,
                    CanInstall = false,
                    ReleaseUrl = AppVersion.GitHubRepoUrl,
                    RepoUrl = AppVersion.GitHubRepoUrl,
                    AssetName = "",
                    AssetUrl = "",
                    Message = "No published releases were found."
                };
            }

            string latestVersion = AppVersion.DisplayVersion(GetString(release, "tag_name"));
            string releaseUrl = GetString(release, "html_url");
            if (string.IsNullOrWhiteSpace(releaseUrl))
            {
                releaseUrl = AppVersion.GitHubRepoUrl;
            }

            Dictionary<string, object> asset = ReleaseAsset(release);
            string assetName = asset != null ? GetString(asset, "name") : "";
            string assetUrl = asset != null ? GetString(asset, "browser_download_url") : "";
            bool updateAvailable = AppVersion.CompareVersions(latestVersion, AppVersion.Current) > 0;
            bool hasInstallAsset = !string.IsNullOrWhiteSpace(assetUrl);
            bool canInstall = updateAvailable && hasInstallAsset && CanInstallUpdates();

            string message;
            if (updateAvailable && hasInstallAsset)
            {
                message = "Update available: " + latestVersion + ".";
            }
            else if (updateAvailable)
            {
                message = "Update available: " + latestVersion + ", but no Windows exe asset was attached.";
            }
            else
            {
                message = "You are on the latest published version (" + latestVersion + ").";
            }

            return new UpdateInfo
            {
                CurrentVersion = AppVersion.Current,
                LatestVersion = latestVersion,
                UpdateAvailable = updateAvailable,
                CanInstall = canInstall,
                ReleaseUrl = releaseUrl,
                RepoUrl = AppVersion.GitHubRepoUrl,
                AssetName = assetName,
                AssetUrl = assetUrl,
                Message = message
            };
        }

        public static string InstallUpdate(UpdateInfo update)
        {
            if (!CanInstallUpdates())
            {
                throw new UpdateException("Self-update is only available in the packaged Windows app.");
            }
            if (update == null || !update.UpdateAvailable)
            {
                throw new UpdateException("No update is available.");
            }
            if (string.IsNullOrWhiteSpace(update.AssetUrl))
            {
                throw new UpdateException("The latest release does not include a Windows exe asset.");
            }

            string targetExe = Path.GetFullPath(Application.ExecutablePath);
            string tempDir = Path.Combine(Path.GetTempPath(), "codex_usage_tray_update_" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(tempDir);
            string newExe = Path.Combine(tempDir, AppVersion.ReleaseAssetName);
            string scriptPath = Path.Combine(tempDir, "update.ps1");

            DownloadFile(update.AssetUrl, newExe);
            WriteUpdaterScript(scriptPath, newExe, targetExe, Process.GetCurrentProcess().Id);
            StartUpdaterScript(scriptPath, targetExe);

            return "Update downloaded. Codex Usage Tray will restart automatically.";
        }

        private static Dictionary<string, object> LatestRelease()
        {
            try
            {
                return GithubJson("/releases/latest");
            }
            catch (WebException ex)
            {
                HttpWebResponse response = ex.Response as HttpWebResponse;
                if (response != null && response.StatusCode == HttpStatusCode.NotFound)
                {
                    response.Close();
                    return null;
                }
                if (response != null)
                {
                    response.Close();
                }
                if (ex.Status == WebExceptionStatus.Timeout)
                {
                    throw new UpdateException("GitHub took too long to respond. Try again in a moment.", ex);
                }

                throw new UpdateException("GitHub update check failed: " + ex.Message, ex);
            }
        }

        private static Dictionary<string, object> GithubJson(string path)
        {
            HttpWebRequest request = (HttpWebRequest)WebRequest.Create(AppVersion.GitHubApiRepo + path);
            request.Accept = "application/vnd.github+json";
            request.UserAgent = "Codex-Usage-Tray";
            request.Headers["X-GitHub-Api-Version"] = "2022-11-28";
            request.Timeout = GitHubTimeoutMilliseconds;

            using (HttpWebResponse response = (HttpWebResponse)request.GetResponse())
            using (Stream stream = response.GetResponseStream())
            using (StreamReader reader = new StreamReader(stream, Encoding.UTF8))
            {
                object parsed = new JavaScriptSerializer().DeserializeObject(reader.ReadToEnd());
                return parsed as Dictionary<string, object>;
            }
        }

        private static Dictionary<string, object> ReleaseAsset(Dictionary<string, object> release)
        {
            object assetsObject;
            if (!release.TryGetValue("assets", out assetsObject) || assetsObject == null)
            {
                return null;
            }

            IEnumerable assets = assetsObject as IEnumerable;
            if (assets == null)
            {
                return null;
            }

            string expectedStem = ComparableExeStem(AppVersion.ReleaseAssetName);
            foreach (object item in assets)
            {
                Dictionary<string, object> asset = item as Dictionary<string, object>;
                if (asset == null)
                {
                    continue;
                }

                if (string.Equals(ComparableExeStem(GetString(asset, "name")), expectedStem, StringComparison.OrdinalIgnoreCase))
                {
                    return asset;
                }
            }

            return null;
        }

        private static string ComparableExeStem(string name)
        {
            if (string.IsNullOrWhiteSpace(name) || !string.Equals(Path.GetExtension(name), ".exe", StringComparison.OrdinalIgnoreCase))
            {
                return "";
            }

            string stem = Path.GetFileNameWithoutExtension(name).ToLowerInvariant();
            StringBuilder builder = new StringBuilder();
            for (int i = 0; i < stem.Length; i++)
            {
                if (char.IsLetterOrDigit(stem[i]))
                {
                    builder.Append(stem[i]);
                }
            }

            return builder.ToString();
        }

        private static string GetString(Dictionary<string, object> values, string key)
        {
            object value;
            if (values != null && values.TryGetValue(key, out value) && value != null)
            {
                return Convert.ToString(value);
            }

            return "";
        }

        private static void DownloadFile(string url, string target)
        {
            using (TimeoutWebClient client = new TimeoutWebClient(DownloadTimeoutMilliseconds))
            {
                client.Headers[HttpRequestHeader.UserAgent] = "Codex-Usage-Tray";
                client.DownloadFile(url, target);
            }
        }

        private static void WriteUpdaterScript(string scriptPath, string newExe, string targetExe, int pid)
        {
            string logPath = Path.ChangeExtension(scriptPath, ".log");
            string[] lines = new string[]
            {
                "$ErrorActionPreference = 'Stop'",
                "$target = " + PowerShellLiteral(targetExe),
                "$newExe = " + PowerShellLiteral(newExe),
                "$appDir = " + PowerShellLiteral(Path.GetDirectoryName(targetExe)),
                "$pidToWait = " + pid,
                "$log = " + PowerShellLiteral(logPath),
                "",
                "function Write-UpdateLog {",
                "  param([string]$Message)",
                "  Add-Content -LiteralPath $log -Value \"$(Get-Date -Format o) $Message\"",
                "}",
                "",
                "function Get-AppProcesses {",
                "  @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $target })",
                "}",
                "",
                "Set-Content -LiteralPath $log -Value \"$(Get-Date -Format o) Starting Codex Usage Tray update\"",
                "try {",
                "  $deadline = (Get-Date).AddSeconds(20)",
                "  while ((Get-Process -Id $pidToWait -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {",
                "    Start-Sleep -Milliseconds 500",
                "  }",
                "",
                "  $remaining = @(Get-AppProcesses)",
                "  if ($remaining.Count -gt 0) {",
                "    Write-UpdateLog \"Stopping $($remaining.Count) old app process(es).\"",
                "    foreach ($process in $remaining) {",
                "      Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue",
                "    }",
                "    Start-Sleep -Milliseconds 800",
                "  }",
                "",
                "  $copied = $false",
                "  for ($attempt = 1; $attempt -le 60; $attempt++) {",
                "    try {",
                "      Copy-Item -LiteralPath $newExe -Destination $target -Force -ErrorAction Stop",
                "      $copied = $true",
                "      break",
                "    } catch {",
                "      Write-UpdateLog \"Copy attempt $attempt failed: $($_.Exception.Message)\"",
                "      Start-Sleep -Seconds 1",
                "    }",
                "  }",
                "  if (-not $copied) {",
                "    throw 'Could not replace the app after 60 attempts.'",
                "  }",
                "",
                "  Write-UpdateLog 'Starting updated app.'",
                "  Start-Process -FilePath $target -WorkingDirectory $appDir",
                "  Remove-Item -LiteralPath $newExe -Force -ErrorAction SilentlyContinue",
                "  Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue",
                "} catch {",
                "  Write-UpdateLog \"Update failed: $($_.Exception.Message)\"",
                "  exit 1",
                "}",
                ""
            };

            File.WriteAllText(scriptPath, string.Join(Environment.NewLine, lines), Encoding.UTF8);
        }

        private static void StartUpdaterScript(string scriptPath, string targetExe)
        {
            ProcessStartInfo startInfo = new ProcessStartInfo();
            startInfo.FileName = "powershell.exe";
            startInfo.Arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File " + QuoteArgument(scriptPath);
            startInfo.WorkingDirectory = Path.GetDirectoryName(targetExe);
            startInfo.UseShellExecute = false;
            startInfo.CreateNoWindow = true;
            startInfo.WindowStyle = ProcessWindowStyle.Hidden;
            Process.Start(startInfo);
        }

        private static string PowerShellLiteral(string value)
        {
            return "'" + (value ?? "").Replace("'", "''") + "'";
        }

        private static string QuoteArgument(string value)
        {
            return "\"" + (value ?? "").Replace("\"", "\\\"") + "\"";
        }

        private sealed class TimeoutWebClient : WebClient
        {
            private readonly int timeoutMilliseconds;

            public TimeoutWebClient(int timeoutMilliseconds)
            {
                this.timeoutMilliseconds = timeoutMilliseconds;
            }

            protected override WebRequest GetWebRequest(Uri address)
            {
                WebRequest request = base.GetWebRequest(address);
                if (request != null)
                {
                    request.Timeout = timeoutMilliseconds;
                }

                return request;
            }
        }
    }
}
