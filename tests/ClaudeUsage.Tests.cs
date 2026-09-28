using System;
using System.IO;

namespace CodexUsageTray.Tests
{
    internal static class ClaudeUsageTests
    {
        public static void Run()
        {
            string directory = Path.Combine(Path.GetTempPath(), "ClaudeUsageTests-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(directory);
            string path = Path.Combine(directory, "usage.json");
            DateTime now = new DateTime(2026, 9, 28, 12, 0, 0, DateTimeKind.Utc);
            try
            {
                TestCaptureAndFreshness(path, now);
                TestAbsentAndExpiredWindows(path, now);
                TestMalformedAndBounds(path, now);
                TestAuthStatus();
            }
            finally
            {
                foreach (string file in Directory.GetFiles(directory)) File.Delete(file);
                Directory.Delete(directory);
            }
        }

        private static void TestCaptureAndFreshness(string path, DateTime now)
        {
            long fiveReset = ToUnixSeconds(now.AddHours(2));
            long weekReset = ToUnixSeconds(now.AddDays(3));
            string input = "{\"session_id\":\"private-session\",\"rate_limits\":{\"five_hour\":{\"used_percentage\":23.5,\"resets_at\":" +
                fiveReset + "},\"seven_day\":{\"used_percentage\":41.2,\"resets_at\":" + weekReset + "}}}";
            Assert(ClaudeUsageSource.CaptureStatusLine(input, path, now), "valid status line is captured");
            string stored = File.ReadAllText(path);
            Assert(!stored.Contains("private-session") && !stored.Contains("session_id") &&
                !stored.Contains("rate_limits"), "cache excludes original status line and identifiers");
            UsageSnapshot snapshot = ClaudeUsageSource.ReadSnapshot(path, now.AddMinutes(30));
            Assert(snapshot != null && snapshot.FiveHour != null && snapshot.Weekly != null,
                "both windows survive through freshness boundary");
            Assert(snapshot.FiveHour.UsedPercent == 23.5 && snapshot.Weekly.UsedPercent == 41.2,
                "usage percentages are preserved");
            Assert(snapshot.FiveHour.WindowMinutes == 300 && snapshot.Weekly.WindowMinutes == 10080,
                "window durations are set");
            Assert(snapshot.FiveHour.ResetAfterSeconds == 7200 &&
                snapshot.LastUpdated.AddSeconds(snapshot.FiveHour.ResetAfterSeconds.Value).ToUniversalTime() == now.AddHours(2),
                "reset deadline remains anchored to capture time");
            Assert(ClaudeUsageSource.ReadSnapshot(path, now.AddMinutes(30).AddTicks(1)) == null,
                "cache expires after 30 minutes");
            Assert(ClaudeUsageSource.ReadSnapshot(path, now.AddTicks(-1)) == null,
                "future capture time is rejected");
        }

        private static void TestAbsentAndExpiredWindows(string path, DateTime now)
        {
            long fiveReset = ToUnixSeconds(now.AddSeconds(10));
            long weekReset = ToUnixSeconds(now.AddDays(3));
            string input = "{\"rate_limits\":{\"five_hour\":{\"used_percentage\":5,\"resets_at\":" +
                fiveReset + "},\"seven_day\":{\"used_percentage\":60,\"resets_at\":" + weekReset + "}}}";
            Assert(ClaudeUsageSource.CaptureStatusLine(input, path, now), "two-window status line is captured");
            UsageSnapshot snapshot = ClaudeUsageSource.ReadSnapshot(path, now.AddSeconds(11));
            Assert(snapshot != null && snapshot.FiveHour == null && snapshot.Weekly != null,
                "expired five-hour window does not hide weekly window");

            string weeklyOnly = "{\"rate_limits\":{\"seven_day\":{\"used_percentage\":0,\"resets_at\":" +
                weekReset + "}}}";
            Assert(ClaudeUsageSource.CaptureStatusLine(weeklyOnly, path, now), "missing five-hour window is accepted");
            snapshot = ClaudeUsageSource.ReadSnapshot(path, now);
            Assert(snapshot != null && snapshot.FiveHour == null && snapshot.Weekly != null,
                "independent weekly window is available");
            Assert(ClaudeUsageSource.CaptureStatusLine("{\"rate_limits\":{}}", path, now),
                "valid absent limits clear old cache");
            Assert(ClaudeUsageSource.ReadSnapshot(path, now) == null, "no current windows yields no snapshot");
        }

        private static void TestMalformedAndBounds(string path, DateTime now)
        {
            long future = ToUnixSeconds(now.AddHours(1));
            string good = "{\"rate_limits\":{\"five_hour\":{\"used_percentage\":100,\"resets_at\":" + future + "}}}";
            Assert(ClaudeUsageSource.CaptureStatusLine(good, path, now), "100 percent is valid");
            string before = File.ReadAllText(path);
            Assert(!ClaudeUsageSource.CaptureStatusLine("{invalid", path, now), "malformed input is rejected");
            Assert(!ClaudeUsageSource.CaptureStatusLine(new string('x', 65537), path, now),
                "oversized input is rejected");
            Assert(File.ReadAllText(path) == before, "invalid input does not overwrite valid cache");
            string outOfBounds = "{\"rate_limits\":{\"five_hour\":{\"used_percentage\":100.1,\"resets_at\":" +
                future + "},\"seven_day\":{\"used_percentage\":-1,\"resets_at\":" + future + "}}}";
            Assert(ClaudeUsageSource.CaptureStatusLine(outOfBounds, path, now), "valid JSON is captured");
            Assert(ClaudeUsageSource.ReadSnapshot(path, now) == null, "out-of-range percentages are ignored");

            string malformedFive = "{\"rate_limits\":{\"five_hour\":{\"used_percentage\":\"9\",\"resets_at\":" +
                future + "},\"seven_day\":{\"used_percentage\":90,\"resets_at\":" + future + "}}}";
            Assert(ClaudeUsageSource.CaptureStatusLine(malformedFive, path, now), "mixed validity is captured");
            UsageSnapshot snapshot = ClaudeUsageSource.ReadSnapshot(path, now);
            Assert(snapshot != null && snapshot.FiveHour == null && snapshot.Weekly != null,
                "malformed five-hour window does not hide weekly window");

            File.WriteAllText(path, "not json");
            Assert(ClaudeUsageSource.ReadSnapshot(path, now) == null, "malformed cache is ignored");
            File.Delete(path);
            Assert(ClaudeUsageSource.ReadSnapshot(path, now) == null, "missing cache is ignored");
        }

        private static void TestAuthStatus()
        {
            Assert(ClaudeUsageSource.ParseAuthStatus(
                "{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"max\"}", 0),
                "subscription login is accepted");
            Assert(!ClaudeUsageSource.ParseAuthStatus("{\"loggedIn\":false}", 0),
                "logged-out account is rejected");
            Assert(!ClaudeUsageSource.ParseAuthStatus("{\"loggedIn\":true}", 1),
                "failed CLI exit is rejected");
            Assert(!ClaudeUsageSource.ParseAuthStatus(
                "{\"loggedIn\":true,\"authMethod\":\"api_key\"}", 0),
                "API key account is rejected");
            Assert(!ClaudeUsageSource.ParseAuthStatus(
                "{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":null}", 0),
                "known absent subscription is rejected");
            Assert(!ClaudeUsageSource.ParseAuthStatus(
                "{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"free\"}", 0),
                "known free account is rejected");
            Assert(!ClaudeUsageSource.ParseAuthStatus(
                "{\"loggedIn\":true,\"apiProvider\":\"bedrock\"}", 0),
                "non-first-party provider is rejected");
        }

        private static long ToUnixSeconds(DateTime time)
        {
            return (long)(time - new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc)).TotalSeconds;
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException(message);
        }
    }
}
