using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;
using System.Threading;

namespace CodexUsageTray.Tests
{
    internal static class CodexHistoryImporterTests
    {
        public static void Run()
        {
            DateTime now = DateTime.UtcNow;
            now = now.AddTicks(-(now.Ticks % TimeSpan.TicksPerSecond));
            string home = Path.Combine(Path.GetTempPath(), "CodexHistoryImporter-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(home);
            try
            {
                TestParsing(now);
                TestScanning(home, now);
                TestIncrementalScanning(home, now);
                TestCancellation(home, now);
            }
            finally
            {
                Directory.Delete(home, true);
            }
        }

        private static void TestParsing(DateTime now)
        {
            DateTime fiveReset = now.AddHours(2);
            DateTime weekReset = now.AddDays(3);
            UsageSnapshot current = Current(now, fiveReset, weekReset);
            string five = Window(12.5, 300, fiveReset);
            string week = Window(37.5, 10080, weekReset);
            UsageSnapshot parsed = CodexHistoryImporter.ParseLine(
                Event(now.AddMinutes(-20), five, week, "codex"), current, now);
            Assert(parsed != null && parsed.FiveHour != null && parsed.Weekly != null,
                "Codex token count event provides both primary limits");
            Assert(parsed.FiveHour.UsedPercent == 12.5 && parsed.Weekly.UsedPercent == 37.5,
                "historical percentages are retained");
            Assert(Math.Abs((parsed.LastUpdated.ToUniversalTime().AddSeconds(parsed.FiveHour.ResetAfterSeconds.Value)
                - fiveReset).TotalSeconds) <= 1, "reset deadline is preserved");

            Assert(CodexHistoryImporter.ParseLine(Event(now.AddMinutes(-20), five, week, "other"), current, now) == null,
                "additional limit IDs are excluded");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddMinutes(-20), five, week, null), current, now) != null,
                "older events without a limit ID are accepted");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddMinutes(-20), Window(20, 300,
                fiveReset.AddMinutes(1)), week, "codex"), current, now).FiveHour == null,
                "different reset cycle is excluded independently");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddMinutes(-20), Window(-1, 300, fiveReset),
                week, "codex"), current, now).FiveHour == null,
                "invalid percentage is excluded independently");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddMinutes(-20), Window(10, 60, fiveReset),
                null, "codex"), current, now) == null,
                "wrong window duration is excluded");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddDays(-29), five, week, "codex"), current, now) == null,
                "samples older than four weeks are excluded");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddSeconds(1), five, week, "codex"), current, now) == null,
                "future samples are excluded");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddMinutes(-5), five, null, "codex"),
                Current(now.AddMinutes(-10), fiveReset, weekReset), now) == null,
                "events newer than the live reading are excluded");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddHours(-4), five, null, "codex"),
                current, now) == null,
                "a reset deadline beyond the five-hour cycle is excluded");
            parsed = CodexHistoryImporter.ParseLine(Event(now.AddDays(-6), null,
                Window(80, 10080, weekReset.AddDays(-7)), "codex"), current, now);
            Assert(parsed != null && parsed.Weekly.UsedPercent == 80,
                "prior reset cycles within 14 days are available for learning");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddDays(-6), null,
                Window(80, 10080, weekReset.AddDays(-7)), "codex"),
                new UsageSnapshot { LastUpdated = now, FiveHour = current.FiveHour }, now) == null,
                "history cannot add a window missing from the current account");
            Assert(CodexHistoryImporter.ParseLine("{invalid", current, now) == null,
                "malformed events are ignored");
            Assert(CodexHistoryImporter.ParseLine(Event(now.AddMinutes(-20), five, week, "codex"),
                new UsageSnapshot(), now) == null, "missing current cycle is rejected");
        }

        private static void TestScanning(string home, DateTime now)
        {
            DateTime fiveReset = now.AddHours(2);
            DateTime weekReset = now.AddDays(3);
            UsageSnapshot current = Current(now, fiveReset, weekReset);
            string sessions = Path.Combine(home, "sessions", "2026", "09", "28");
            string archived = Path.Combine(home, "archived_sessions");
            Directory.CreateDirectory(sessions);
            Directory.CreateDirectory(archived);
            string first = Event(now.AddMinutes(-40), Window(5, 300, fiveReset), null, "codex");
            string second = Event(now.AddMinutes(-20), Window(25, 300, fiveReset), null, "codex");
            string duplicate = Event(now.AddMinutes(-20), null, Window(40, 10080, weekReset), "codex");
            File.WriteAllText(Path.Combine(sessions, "live.jsonl"),
                "{invalid\n" + new string('x', 65537) + "\n" + second + "\n" + first + "\n",
                new UTF8Encoding(false));
            File.WriteAllText(Path.Combine(archived, "old.jsonl"), duplicate,
                new UTF8Encoding(false));
            List<UsageSnapshot> snapshots = CodexHistoryImporter.ReadSnapshots(home, current, now,
                CancellationToken.None);
            Assert(snapshots.Count == 2, "sessions and archived sessions are scanned and duplicates merged");
            Assert(snapshots[0].LastUpdated.ToUniversalTime() == now.AddMinutes(-40) &&
                snapshots[1].LastUpdated.ToUniversalTime() == now.AddMinutes(-20),
                "samples are sorted by event time");
            Assert(snapshots[1].FiveHour != null && snapshots[1].Weekly != null,
                "duplicate event without a trailing newline retains independently available windows");
            Assert(CodexHistoryImporter.ReadSnapshots(Path.Combine(home, "missing"), current, now,
                CancellationToken.None).Count == 0, "missing session directories are harmless");
        }

        private static void TestCancellation(string home, DateTime now)
        {
            CancellationTokenSource source = new CancellationTokenSource();
            source.Cancel();
            Assert(CodexHistoryImporter.ReadSnapshots(home, Current(now, now.AddHours(2),
                now.AddDays(3)), now, source.Token).Count == 0,
                "pre-canceled scan returns no samples");
        }

        private static void TestIncrementalScanning(string home, DateTime now)
        {
            string directory = Path.Combine(home, "incremental", "sessions");
            Directory.CreateDirectory(directory);
            string path = Path.Combine(directory, "append.jsonl");
            DateTime reset = now.AddDays(3);
            UsageSnapshot current = Current(now, now.AddHours(2), reset);
            CodexHistoryImporter importer = new CodexHistoryImporter();
            string first = Event(now.AddMinutes(-20), null, Window(20, 10080, reset), "codex") + "\n";
            string second = Event(now.AddMinutes(-10), null, Window(21, 10080, reset), "codex") + "\n";
            File.WriteAllText(path, first, new UTF8Encoding(false));
            string root = Path.GetDirectoryName(directory);
            Assert(importer.ReadUpdates(root, current, now, CancellationToken.None).Count == 1,
                "initial incremental scan reads the existing log");
            Assert(importer.ReadUpdates(root, current, now, CancellationToken.None).Count == 0 &&
                importer.LastScanBytes == 0, "unchanged logs are not reread");
            File.AppendAllText(path, second.Substring(0, second.Length / 2), new UTF8Encoding(false));
            Assert(importer.ReadUpdates(root, current, now, CancellationToken.None).Count == 0,
                "partial appended lines wait until the writer finishes");
            File.AppendAllText(path, second.Substring(second.Length / 2), new UTF8Encoding(false));
            Assert(importer.ReadUpdates(root, current, now, CancellationToken.None).Count == 1 &&
                importer.LastScanBytes == Encoding.UTF8.GetByteCount(second),
                "completed appended lines are read without rescanning the prefix");
            File.WriteAllText(path, first, new UTF8Encoding(false));
            Assert(importer.ReadUpdates(root, current, now, CancellationToken.None).Count == 1,
                "truncated logs restart from the beginning");
        }

        private static UsageSnapshot Current(DateTime now, DateTime fiveReset, DateTime weekReset)
        {
            return new UsageSnapshot
            {
                LastUpdated = now.ToLocalTime(),
                FiveHour = new LimitWindow
                {
                    UsedPercent = 50,
                    WindowMinutes = 300,
                    ResetAfterSeconds = (int)(fiveReset - now).TotalSeconds
                },
                Weekly = new LimitWindow
                {
                    UsedPercent = 60,
                    WindowMinutes = 10080,
                    ResetAfterSeconds = (int)(weekReset - now).TotalSeconds
                }
            };
        }

        private static string Event(DateTime timestamp, string primary, string secondary, string limitId)
        {
            return "{\"timestamp\":\"" + timestamp.ToString("o", CultureInfo.InvariantCulture) +
                "\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"rate_limits\":{" +
                (limitId == null ? "" : "\"limit_id\":\"" + limitId + "\",") +
                "\"primary\":" + (primary ?? "null") + ",\"secondary\":" + (secondary ?? "null") + "}}}";
        }

        private static string Window(double percent, int minutes, DateTime reset)
        {
            long unixSeconds = (long)(reset - new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc)).TotalSeconds;
            return "{\"used_percent\":" + percent.ToString(CultureInfo.InvariantCulture) +
                ",\"window_minutes\":" + minutes + ",\"resets_at\":" + unixSeconds + "}";
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("Codex history importer: " + message);
        }
    }
}
