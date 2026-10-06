using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;

namespace CodexUsageTray
{
    internal static class ClaudeUsageSource
    {
        private const int MaxInputCharacters = 65536;
        private const int MaxCacheBytes = 4096;
        private const int MaxAuthOutputCharacters = 32768;
        private const int AuthTimeoutMilliseconds = 3000;
        private const int FreshnessMinutes = 30;
        private static readonly DateTime UnixEpoch = new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc);

        internal static string DefaultPath
        {
            get
            {
                return Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                    "CodexUsageTray", "claude-usage.json");
            }
        }

        internal static void HandleStatusLine()
        {
            try
            {
                char[] buffer = new char[MaxInputCharacters + 1];
                int total = 0;
                while (total < buffer.Length)
                {
                    int count = Console.In.Read(buffer, total, buffer.Length - total);
                    if (count == 0)
                    {
                        break;
                    }
                    total += count;
                }
                if (total > MaxInputCharacters)
                {
                    return;
                }
                CaptureStatusLine(new string(buffer, 0, total), DefaultPath, DateTime.UtcNow);
            }
            catch (Exception)
            {
                // A status line invocation must never interrupt Claude Code.
            }
        }

        internal static bool CaptureStatusLine(string input, string path, DateTime nowUtc)
        {
            if (string.IsNullOrWhiteSpace(input) || input.Length > MaxInputCharacters ||
                string.IsNullOrWhiteSpace(path) || nowUtc.Kind != DateTimeKind.Utc)
            {
                return false;
            }

            Dictionary<string, object> root = ParseObject(input);
            if (root == null)
            {
                return false;
            }

            Dictionary<string, object> limits = GetObject(root, "rate_limits");
            // Startup status lines have no quota data until the first API response.
            // Keep the last actual reading and its original capture time.
            if (limits == null) return false;
            LimitWindowData fiveHour = ReadWindow(limits, "five_hour", nowUtc);
            LimitWindowData weekly = ReadWindow(limits, "seven_day", nowUtc);
            Dictionary<string, object> cache = new Dictionary<string, object>();
            cache["captured_at_utc_ticks"] = nowUtc.Ticks;
            if (fiveHour != null) cache["five_hour"] = SerializeWindow(fiveHour);
            if (weekly != null) cache["seven_day"] = SerializeWindow(weekly);

            string temporaryPath = null;
            try
            {
                string directory = Path.GetDirectoryName(Path.GetFullPath(path));
                Directory.CreateDirectory(directory);
                temporaryPath = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
                string json = new JavaScriptSerializer().Serialize(cache);
                File.WriteAllText(temporaryPath, json, new UTF8Encoding(false));
                for (int attempt = 0; ; attempt++)
                {
                    try
                    {
                        if (File.Exists(path)) File.Replace(temporaryPath, path, null);
                        else File.Move(temporaryPath, path);
                        break;
                    }
                    catch (IOException)
                    {
                        if (attempt >= 2) throw;
                        System.Threading.Thread.Sleep(25);
                    }
                }
                temporaryPath = null;
                return true;
            }
            catch (Exception)
            {
                return false;
            }
            finally
            {
                if (temporaryPath != null)
                {
                    try { File.Delete(temporaryPath); }
                    catch (Exception) { }
                }
            }
        }

        internal static UsageSnapshot ReadSnapshot(string path, DateTime nowUtc)
        {
            if (string.IsNullOrWhiteSpace(path) || nowUtc.Kind != DateTimeKind.Utc)
            {
                return null;
            }
            try
            {
                FileInfo file = new FileInfo(path);
                if (!file.Exists || file.Length > MaxCacheBytes)
                {
                    return null;
                }
                Dictionary<string, object> cache = ParseObject(File.ReadAllText(path));
                if (cache == null)
                {
                    return null;
                }
                long capturedTicks;
                object capturedObject;
                if (!cache.TryGetValue("captured_at_utc_ticks", out capturedObject) ||
                    !TryGetLong(capturedObject, out capturedTicks) ||
                    capturedTicks < DateTime.MinValue.Ticks || capturedTicks > DateTime.MaxValue.Ticks)
                {
                    return null;
                }
                DateTime capturedAt = new DateTime(capturedTicks, DateTimeKind.Utc);
                TimeSpan age = nowUtc - capturedAt;
                if (age < TimeSpan.Zero)
                {
                    return null;
                }

                LimitWindowData fiveHour = ReadCachedWindow(cache, "five_hour", nowUtc);
                LimitWindowData weekly = ReadCachedWindow(cache, "seven_day", nowUtc);
                if (fiveHour == null && weekly == null)
                {
                    return null;
                }
                UsageSnapshot snapshot = new UsageSnapshot();
                snapshot.LastUpdated = capturedAt.ToLocalTime();
                snapshot.LastAttempted = nowUtc.ToLocalTime();
                snapshot.IsStale = age > TimeSpan.FromMinutes(FreshnessMinutes);
                snapshot.FiveHour = ToLimitWindow(fiveHour, 300, capturedAt);
                snapshot.Weekly = ToLimitWindow(weekly, 10080, capturedAt);
                return snapshot;
            }
            catch (Exception)
            {
                return null;
            }
        }

        internal static bool IsAuthenticated()
        {
            string executable = FindClaudeExecutable();
            if (executable == null)
            {
                return false;
            }
            try
            {
                using (Process process = new Process())
                {
                    process.StartInfo = new ProcessStartInfo(executable, "auth status");
                    process.StartInfo.UseShellExecute = false;
                    process.StartInfo.CreateNoWindow = true;
                    process.StartInfo.RedirectStandardOutput = true;
                    process.StartInfo.RedirectStandardError = true;
                    StringBuilder output = new StringBuilder();
                    int outputDone = 0;
                    int errorDone = 0;
                    int outputTooLarge = 0;
                    process.OutputDataReceived += delegate(object sender, DataReceivedEventArgs args)
                    {
                        if (args.Data == null)
                        {
                            Interlocked.Exchange(ref outputDone, 1);
                        }
                        else if (output.Length + args.Data.Length <= MaxAuthOutputCharacters)
                        {
                            output.Append(args.Data);
                        }
                        else
                        {
                            Interlocked.Exchange(ref outputTooLarge, 1);
                        }
                    };
                    process.ErrorDataReceived += delegate(object sender, DataReceivedEventArgs args)
                    {
                        if (args.Data == null) Interlocked.Exchange(ref errorDone, 1);
                    };
                    process.Start();
                    process.BeginOutputReadLine();
                    process.BeginErrorReadLine();
                    if (!process.WaitForExit(AuthTimeoutMilliseconds))
                    {
                        try { process.Kill(); }
                        catch (Exception) { }
                        return false;
                    }
                    if (!SpinWait.SpinUntil(delegate
                        {
                            return Thread.VolatileRead(ref outputDone) != 0 &&
                                Thread.VolatileRead(ref errorDone) != 0;
                        }, 500) || Thread.VolatileRead(ref outputTooLarge) != 0)
                    {
                        return false;
                    }
                    return ParseAuthStatus(output.ToString(), process.ExitCode);
                }
            }
            catch (Exception)
            {
                return false;
            }
        }

        internal static bool ParseAuthStatus(string json, int exitCode)
        {
            if (exitCode != 0 || string.IsNullOrWhiteSpace(json) ||
                json.Length > MaxAuthOutputCharacters)
            {
                return false;
            }
            Dictionary<string, object> root = ParseObject(json);
            object loggedIn;
            if (root == null || !root.TryGetValue("loggedIn", out loggedIn) ||
                !(loggedIn is bool) || !(bool)loggedIn)
            {
                return false;
            }
            object method;
            if (root.TryGetValue("authMethod", out method) &&
                !string.Equals(method as string, "claude.ai", StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }
            object provider;
            if (root.TryGetValue("apiProvider", out provider) &&
                !string.Equals(provider as string, "firstParty", StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }
            object subscription;
            if (root.TryGetValue("subscriptionType", out subscription))
            {
                string type = subscription as string;
                if (string.IsNullOrWhiteSpace(type) ||
                    string.Equals(type, "none", StringComparison.OrdinalIgnoreCase) ||
                    string.Equals(type, "free", StringComparison.OrdinalIgnoreCase))
                {
                    return false;
                }
            }
            return true;
        }

        private static string FindClaudeExecutable()
        {
            string home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
            string native = Path.Combine(home, ".local", "bin", "claude.exe");
            if (File.Exists(native))
            {
                return native;
            }
            string path = Environment.GetEnvironmentVariable("PATH") ?? "";
            foreach (string directory in path.Split(Path.PathSeparator))
            {
                if (string.IsNullOrWhiteSpace(directory)) continue;
                try
                {
                    string candidate = Path.Combine(directory.Trim().Trim('"'), "claude.exe");
                    if (File.Exists(candidate)) return candidate;
                }
                catch (Exception) { }
            }
            return null;
        }

        private static Dictionary<string, object> ParseObject(string json)
        {
            try
            {
                return new JavaScriptSerializer { MaxJsonLength = MaxInputCharacters + 1 }
                    .DeserializeObject(json) as Dictionary<string, object>;
            }
            catch (Exception)
            {
                return null;
            }
        }

        private static Dictionary<string, object> GetObject(Dictionary<string, object> source, string key)
        {
            object value;
            return source != null && source.TryGetValue(key, out value)
                ? value as Dictionary<string, object> : null;
        }

        private static LimitWindowData ReadWindow(Dictionary<string, object> source, string key, DateTime nowUtc)
        {
            Dictionary<string, object> value = GetObject(source, key);
            if (value == null) return null;
            object usedObject;
            object resetObject;
            double used;
            long reset;
            if (!value.TryGetValue("used_percentage", out usedObject) ||
                !value.TryGetValue("resets_at", out resetObject) ||
                !TryGetPercentage(usedObject, out used) ||
                !TryGetLong(resetObject, out reset) ||
                !IsFutureReset(reset, nowUtc))
            {
                return null;
            }
            return new LimitWindowData { UsedPercent = used, ResetAtUnixSeconds = reset };
        }

        private static LimitWindowData ReadCachedWindow(Dictionary<string, object> source, string key, DateTime nowUtc)
        {
            Dictionary<string, object> value = GetObject(source, key);
            if (value == null) return null;
            object usedObject;
            object resetObject;
            double used;
            long reset;
            if (!value.TryGetValue("used_percent", out usedObject) ||
                !value.TryGetValue("reset_at_unix_seconds", out resetObject) ||
                !TryGetPercentage(usedObject, out used) ||
                !TryGetLong(resetObject, out reset) ||
                !IsFutureReset(reset, nowUtc))
            {
                return null;
            }
            return new LimitWindowData { UsedPercent = used, ResetAtUnixSeconds = reset };
        }

        private static Dictionary<string, object> SerializeWindow(LimitWindowData value)
        {
            return new Dictionary<string, object>
            {
                { "used_percent", value.UsedPercent },
                { "reset_at_unix_seconds", value.ResetAtUnixSeconds }
            };
        }

        private static LimitWindow ToLimitWindow(LimitWindowData value, int minutes, DateTime capturedAt)
        {
            if (value == null) return null;
            TimeSpan remaining = UnixEpoch.AddSeconds(value.ResetAtUnixSeconds) - capturedAt;
            return new LimitWindow
            {
                UsedPercent = value.UsedPercent,
                WindowMinutes = minutes,
                ResetAfterSeconds = remaining.TotalSeconds <= int.MaxValue
                    ? (int)Math.Ceiling(remaining.TotalSeconds) : (int?)null
            };
        }

        private static bool TryGetPercentage(object value, out double percentage)
        {
            percentage = 0;
            if (!(value is int) && !(value is long) && !(value is decimal) && !(value is double))
            {
                return false;
            }
            try { percentage = Convert.ToDouble(value, CultureInfo.InvariantCulture); }
            catch (Exception) { return false; }
            return !double.IsNaN(percentage) && !double.IsInfinity(percentage) &&
                percentage >= 0 && percentage <= 100;
        }

        private static bool TryGetLong(object value, out long number)
        {
            number = 0;
            if (!(value is int) && !(value is long) && !(value is decimal) && !(value is double))
            {
                return false;
            }
            try
            {
                decimal numeric = Convert.ToDecimal(value, CultureInfo.InvariantCulture);
                if (numeric != decimal.Truncate(numeric)) return false;
                number = decimal.ToInt64(numeric);
                return true;
            }
            catch (Exception) { return false; }
        }

        private static bool IsFutureReset(long resetAtUnixSeconds, DateTime nowUtc)
        {
            try { return UnixEpoch.AddSeconds(resetAtUnixSeconds) > nowUtc; }
            catch (ArgumentOutOfRangeException) { return false; }
        }

        private sealed class LimitWindowData
        {
            public double UsedPercent;
            public long ResetAtUnixSeconds;
        }
    }
}
