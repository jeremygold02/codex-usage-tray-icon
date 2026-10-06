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
    internal sealed class CodexHistoryImporter
    {
        private const int MaxLineBytes = 65536;
        private const int MaxSamples = 180000;
        private const long MaxScanBytes = 8L * 1024 * 1024 * 1024;
        private const int MaxScanMilliseconds = 30000;
        private static readonly DateTime UnixEpoch = new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc);
        private static readonly Encoding Utf8 = new UTF8Encoding(false, true);
        private static readonly byte[] TokenCountMarker = Encoding.ASCII.GetBytes("token_count");
        private static readonly byte[] RateLimitsMarker = Encoding.ASCII.GetBytes("rate_limits");
        private sealed class FileCursor
        {
            public long Offset;
            public long Length;
            public DateTime ModifiedUtc;
        }
        private readonly Dictionary<string, FileCursor> cursors =
            new Dictionary<string, FileCursor>(StringComparer.OrdinalIgnoreCase);

        internal long LastScanBytes { get; private set; }
        internal bool LastScanCompleted { get; private set; }

        internal static string DefaultCodexHome
        {
            get
            {
                string configured = Environment.GetEnvironmentVariable("CODEX_HOME");
                return !string.IsNullOrWhiteSpace(configured) ? configured :
                    Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".codex");
            }
        }

        internal static List<UsageSnapshot> ReadSnapshots(
            string codexHome, UsageSnapshot current, DateTime nowUtc, CancellationToken cancellation)
        {
            return new CodexHistoryImporter().ReadUpdates(codexHome, current, nowUtc, cancellation);
        }

        internal List<UsageSnapshot> ReadUpdates(
            string codexHome, UsageSnapshot current, DateTime nowUtc, CancellationToken cancellation)
        {
            LastScanBytes = 0;
            LastScanCompleted = false;
            List<UsageSnapshot> result = new List<UsageSnapshot>();
            if (string.IsNullOrWhiteSpace(codexHome) || nowUtc.Kind != DateTimeKind.Utc ||
                cancellation.IsCancellationRequested || !HasCurrentCycle(current, nowUtc))
            {
                return result;
            }

            Stopwatch clock = Stopwatch.StartNew();
            DateTime earliest = nowUtc.AddDays(-UsageHistoryStore.RetentionDays);
            List<FileInfo> files = FindRecentFiles(codexHome, earliest, clock, cancellation);
            files.Sort(delegate(FileInfo left, FileInfo right)
            {
                return right.LastWriteTimeUtc.CompareTo(left.LastWriteTimeUtc);
            });

            SortedDictionary<DateTime, UsageSnapshot> samples = new SortedDictionary<DateTime, UsageSnapshot>();
            long scannedBytes = 0;
            int processed = 0;
            foreach (FileInfo file in files)
            {
                if (ShouldStop(clock, cancellation) || scannedBytes >= MaxScanBytes) break;
                FileCursor cursor;
                if (!cursors.TryGetValue(file.FullName, out cursor))
                {
                    cursor = new FileCursor();
                    cursors.Add(file.FullName, cursor);
                }
                if (cursor.Length == file.Length && cursor.ModifiedUtc == file.LastWriteTimeUtc &&
                    cursor.Offset == file.Length)
                {
                    processed++;
                    continue;
                }
                if (file.Length < cursor.Length || (file.Length == cursor.Length &&
                    cursor.ModifiedUtc != file.LastWriteTimeUtc)) cursor.Offset = 0;
                cursor.Offset = ScanFile(file.FullName, cursor.Offset, current, nowUtc, clock,
                    cancellation, samples, ref scannedBytes);
                cursor.Length = file.Length;
                cursor.ModifiedUtc = file.LastWriteTimeUtc;
                processed++;
            }
            // Offsets live only for recent logs; neither prompts nor log contents are cached.
            HashSet<string> recentPaths = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (FileInfo file in files) recentPaths.Add(file.FullName);
            foreach (string path in new List<string>(cursors.Keys))
                if (!recentPaths.Contains(path)) cursors.Remove(path);
            LastScanBytes = scannedBytes;
            LastScanCompleted = processed == files.Count && !ShouldStop(clock, cancellation) && scannedBytes < MaxScanBytes;
            foreach (UsageSnapshot snapshot in samples.Values) result.Add(snapshot);
            return result;
        }

        internal static UsageSnapshot ParseLine(string line, UsageSnapshot current, DateTime nowUtc)
        {
            if (string.IsNullOrWhiteSpace(line) || line.Length > MaxLineBytes ||
                nowUtc.Kind != DateTimeKind.Utc || !HasCurrentCycle(current, nowUtc))
            {
                return null;
            }
            try
            {
                Dictionary<string, object> root = new JavaScriptSerializer { MaxJsonLength = MaxLineBytes }
                    .DeserializeObject(line.TrimStart('\uFEFF')) as Dictionary<string, object>;
                if (root == null || !HasString(root, "type", "event_msg")) return null;
                Dictionary<string, object> payload = GetObject(root, "payload");
                if (payload == null || !HasString(payload, "type", "token_count")) return null;
                Dictionary<string, object> limits = GetObject(payload, "rate_limits");
                if (limits == null) return null;
                object limitId;
                if (limits.TryGetValue("limit_id", out limitId) &&
                    !string.Equals(limitId as string, "codex", StringComparison.OrdinalIgnoreCase))
                {
                    return null;
                }

                object timestampObject;
                DateTimeOffset timestamp;
                if (!root.TryGetValue("timestamp", out timestampObject) ||
                    !(timestampObject is string) ||
                    !DateTimeOffset.TryParse((string)timestampObject, CultureInfo.InvariantCulture,
                        DateTimeStyles.RoundtripKind, out timestamp))
                {
                    return null;
                }
                string timestampText = (string)timestampObject;
                if (!timestampText.EndsWith("Z", StringComparison.OrdinalIgnoreCase) &&
                    !timestampText.EndsWith("+00:00", StringComparison.Ordinal)) return null;
                DateTime eventUtc = timestamp.UtcDateTime;
                if (eventUtc < nowUtc.AddDays(-UsageHistoryStore.RetentionDays) || eventUtc > nowUtc ||
                    eventUtc > current.LastUpdated.ToUniversalTime()) return null;

                DateTime? currentFiveReset = GetCurrentReset(current.FiveHour, 300, nowUtc, current.LastUpdated);
                DateTime? currentWeekReset = GetCurrentReset(current.Weekly, 10080, nowUtc, current.LastUpdated);
                LimitWindow fiveHour = null;
                LimitWindow weekly = null;
                ReadHistoricalWindow(GetObject(limits, "primary"), eventUtc,
                    currentFiveReset, currentWeekReset, ref fiveHour, ref weekly);
                ReadHistoricalWindow(GetObject(limits, "secondary"), eventUtc,
                    currentFiveReset, currentWeekReset, ref fiveHour, ref weekly);
                if (fiveHour == null && weekly == null) return null;
                return new UsageSnapshot
                {
                    LastUpdated = eventUtc.ToLocalTime(),
                    LastAttempted = eventUtc.ToLocalTime(),
                    FiveHour = fiveHour,
                    Weekly = weekly
                };
            }
            catch (Exception)
            {
                return null;
            }
        }

        private static List<FileInfo> FindRecentFiles(
            string codexHome, DateTime earliest, Stopwatch clock, CancellationToken cancellation)
        {
            List<FileInfo> files = new List<FileInfo>();
            foreach (string subdirectory in new[] { "sessions", "archived_sessions" })
            {
                string root = Path.Combine(codexHome, subdirectory);
                Stack<string> pending = new Stack<string>();
                pending.Push(root);
                while (pending.Count > 0 && !ShouldStop(clock, cancellation))
                {
                    string directory = pending.Pop();
                    try
                    {
                        if (!Directory.Exists(directory)) continue;
                        foreach (string path in Directory.EnumerateFiles(directory, "*.jsonl", SearchOption.TopDirectoryOnly))
                        {
                            if (ShouldStop(clock, cancellation)) break;
                            try
                            {
                                FileInfo file = new FileInfo(path);
                                if (file.LastWriteTimeUtc >= earliest) files.Add(file);
                            }
                            catch (Exception) { }
                        }
                        foreach (string child in Directory.EnumerateDirectories(directory, "*", SearchOption.TopDirectoryOnly))
                        {
                            if (ShouldStop(clock, cancellation)) break;
                            try
                            {
                                if ((File.GetAttributes(child) & FileAttributes.ReparsePoint) == 0)
                                    pending.Push(child);
                            }
                            catch (Exception) { }
                        }
                    }
                    catch (Exception) { }
                }
            }
            return files;
        }

        private static long ScanFile(string path, long startOffset, UsageSnapshot current, DateTime nowUtc,
            Stopwatch clock, CancellationToken cancellation,
            SortedDictionary<DateTime, UsageSnapshot> samples, ref long scannedBytes)
        {
            long completedOffset = startOffset;
            try
            {
                using (FileStream input = new FileStream(path, FileMode.Open, FileAccess.Read,
                    FileShare.ReadWrite | FileShare.Delete, 8192, FileOptions.SequentialScan))
                {
                    input.Seek(startOffset, SeekOrigin.Begin);
                    byte[] buffer = new byte[8192];
                    byte[] line = new byte[MaxLineBytes];
                    int lineLength = 0;
                    bool oversized = false;
                    while (!ShouldStop(clock, cancellation) && scannedBytes < MaxScanBytes)
                    {
                        int allowed = (int)Math.Min(buffer.Length, MaxScanBytes - scannedBytes);
                        int read = input.Read(buffer, 0, allowed);
                        if (read == 0) break;
                        scannedBytes += read;
                        int offset = 0;
                        while (offset < read)
                        {
                            int newline = Array.IndexOf(buffer, (byte)'\n', offset, read - offset);
                            int end = newline >= 0 ? newline : read;
                            int segmentLength = end - offset;
                            if (!oversized)
                            {
                                if (lineLength + segmentLength <= MaxLineBytes)
                                {
                                    Buffer.BlockCopy(buffer, offset, line, lineLength, segmentLength);
                                    lineLength += segmentLength;
                                }
                                else oversized = true;
                            }
                            if (newline >= 0)
                            {
                                if (!oversized && lineLength > 0)
                                    AddLine(line, lineLength, current, nowUtc, samples);
                                lineLength = 0;
                                oversized = false;
                                offset = newline + 1;
                                completedOffset = input.Position - read + offset;
                            }
                            else offset = read;
                        }
                    }
                    // Parse a complete final JSON value even without a newline. Keep
                    // its offset uncommitted so a partial append can be retried safely.
                    if (!oversized && lineLength > 0 && !ShouldStop(clock, cancellation))
                        AddLine(line, lineLength, current, nowUtc, samples);
                }
            }
            catch (Exception) { }
            return completedOffset;
        }

        private static void AddLine(byte[] line, int length, UsageSnapshot current,
            DateTime nowUtc, SortedDictionary<DateTime, UsageSnapshot> samples)
        {
            if (!Contains(line, length, TokenCountMarker) || !Contains(line, length, RateLimitsMarker))
                return;
            string json;
            try { json = Utf8.GetString(line, 0, length); }
            catch (DecoderFallbackException) { return; }
            UsageSnapshot parsed = ParseLine(json, current, nowUtc);
            if (parsed == null) return;
            DateTime timestamp = parsed.LastUpdated.ToUniversalTime();
            UsageSnapshot existing;
            if (samples.TryGetValue(timestamp, out existing))
            {
                if (existing.FiveHour == null) existing.FiveHour = parsed.FiveHour;
                if (existing.Weekly == null) existing.Weekly = parsed.Weekly;
                return;
            }
            samples.Add(timestamp, parsed);
            if (samples.Count > MaxSamples)
            {
                DateTime oldest = DateTime.MinValue;
                foreach (DateTime key in samples.Keys) { oldest = key; break; }
                samples.Remove(oldest);
            }
        }

        private static bool Contains(byte[] line, int length, byte[] marker)
        {
            for (int i = 0; i <= length - marker.Length; i++)
            {
                int j = 0;
                while (j < marker.Length && line[i + j] == marker[j]) j++;
                if (j == marker.Length) return true;
            }
            return false;
        }

        private static void ReadHistoricalWindow(Dictionary<string, object> source, DateTime eventUtc,
            DateTime? currentFiveReset, DateTime? currentWeekReset,
            ref LimitWindow fiveHour, ref LimitWindow weekly)
        {
            if (source == null) return;
            object minutesObject;
            object usedObject;
            object resetObject;
            int minutes;
            double used;
            long resetSeconds;
            if (!source.TryGetValue("window_minutes", out minutesObject) ||
                !source.TryGetValue("used_percent", out usedObject) ||
                !source.TryGetValue("resets_at", out resetObject) ||
                !TryGetInt(minutesObject, out minutes) ||
                !TryGetPercent(usedObject, out used) ||
                !TryGetLong(resetObject, out resetSeconds)) return;
            DateTime resetUtc;
            try { resetUtc = UnixEpoch.AddSeconds(resetSeconds); }
            catch (ArgumentOutOfRangeException) { return; }
            if (resetUtc <= eventUtc) return;
            DateTime? expected = minutes == 300 ? currentFiveReset :
                minutes == 10080 ? currentWeekReset : null;
            // Retain earlier cycles for learning; the chart applies its own reset boundary.
            // Exclude windows absent from the signed-in account and future reset cycles.
            if (!expected.HasValue || resetUtc > expected.Value.AddSeconds(2)) return;
            double remaining = (resetUtc - eventUtc).TotalSeconds;
            if (remaining > minutes * 60 + 2) return;
            LimitWindow window = new LimitWindow
            {
                UsedPercent = used,
                WindowMinutes = minutes,
                ResetAfterSeconds = (int)Math.Ceiling(remaining)
            };
            if (minutes == 300 && fiveHour == null) fiveHour = window;
            if (minutes == 10080 && weekly == null) weekly = window;
        }

        private static bool HasCurrentCycle(UsageSnapshot current, DateTime nowUtc)
        {
            return current != null && current.HasPrimaryLimit &&
                (GetCurrentReset(current.FiveHour, 300, nowUtc, current.LastUpdated).HasValue ||
                 GetCurrentReset(current.Weekly, 10080, nowUtc, current.LastUpdated).HasValue);
        }

        private static DateTime? GetCurrentReset(LimitWindow window, int minutes, DateTime nowUtc, DateTime updated)
        {
            if (window == null || window.WindowMinutes != minutes ||
                !window.ResetAfterSeconds.HasValue || updated == DateTime.MinValue) return null;
            try
            {
                DateTime reset = updated.ToUniversalTime().AddSeconds(window.ResetAfterSeconds.Value);
                return reset > nowUtc ? (DateTime?)reset : null;
            }
            catch (ArgumentOutOfRangeException) { return null; }
        }

        private static Dictionary<string, object> GetObject(Dictionary<string, object> source, string key)
        {
            object value;
            return source != null && source.TryGetValue(key, out value)
                ? value as Dictionary<string, object> : null;
        }

        private static bool HasString(Dictionary<string, object> source, string key, string expected)
        {
            object value;
            return source.TryGetValue(key, out value) &&
                string.Equals(value as string, expected, StringComparison.Ordinal);
        }

        private static bool TryGetPercent(object value, out double percent)
        {
            percent = 0;
            if (!(value is int) && !(value is long) && !(value is decimal) && !(value is double)) return false;
            try { percent = Convert.ToDouble(value, CultureInfo.InvariantCulture); }
            catch (Exception) { return false; }
            return !double.IsNaN(percent) && !double.IsInfinity(percent) && percent >= 0 && percent <= 100;
        }

        private static bool TryGetInt(object value, out int number)
        {
            long result;
            number = 0;
            if (!TryGetLong(value, out result) || result < int.MinValue || result > int.MaxValue) return false;
            number = (int)result;
            return true;
        }

        private static bool TryGetLong(object value, out long number)
        {
            number = 0;
            if (!(value is int) && !(value is long) && !(value is decimal) && !(value is double)) return false;
            try
            {
                decimal numeric = Convert.ToDecimal(value, CultureInfo.InvariantCulture);
                if (numeric != decimal.Truncate(numeric)) return false;
                number = decimal.ToInt64(numeric);
                return true;
            }
            catch (Exception) { return false; }
        }

        private static bool ShouldStop(Stopwatch clock, CancellationToken cancellation)
        {
            return cancellation.IsCancellationRequested || clock.ElapsedMilliseconds >= MaxScanMilliseconds;
        }
    }
}
