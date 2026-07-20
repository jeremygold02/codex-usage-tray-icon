using System;
using System.IO;

namespace CodexUsageTray.Tests
{
    internal static class Program
    {
        private static int failures;

        private static int Main(string[] args)
        {
            if (args == null || args.Length != 1)
            {
                Console.Error.WriteLine("Usage: CodexUsageTray.Tests.exe <rate-limits-fixture.json>");
                return 2;
            }

            TestRateLimitFixture(args[0]);
            TestMalformedResponses();
            TestPartialPrimaryLimits();
            TestMissingPrimaryLimits();
            TestRpcErrors();
            TestDeepClone();

            if (failures != 0)
            {
                Console.Error.WriteLine(failures + " test assertion(s) failed.");
                return 1;
            }

            Console.WriteLine("All Codex Usage Tray tests passed.");
            return 0;
        }

        private static void TestRateLimitFixture(string fixturePath)
        {
            string json = File.ReadAllText(fixturePath);
            DateTime attemptedAt = new DateTime(2026, 7, 10, 16, 31, 47, DateTimeKind.Utc);
            UsageSnapshot snapshot = CodexRateLimitClient.ParseRateLimitsResponse(json, attemptedAt);

            AssertTrue(string.IsNullOrEmpty(snapshot.ErrorMessage), "fixture parses without an error");
            AssertTrue(snapshot.HasAnyLimit, "fixture has usage limits");
            AssertEqual(300, snapshot.FiveHour.WindowMinutes.GetValueOrDefault(), "5-hour duration");
            AssertEqual(10080, snapshot.Weekly.WindowMinutes.GetValueOrDefault(), "weekly duration");
            AssertEqual(80, snapshot.FiveHour.RemainingPercent, "5-hour remaining percentage");
            AssertEqual(78, snapshot.Weekly.RemainingPercent, "weekly remaining percentage");
            AssertTrue(snapshot.FiveHour.ResetAfterSeconds.GetValueOrDefault() > 0, "5-hour reset is in the future");
            AssertTrue(
                snapshot.Weekly.ResetAfterSeconds.GetValueOrDefault() > snapshot.FiveHour.ResetAfterSeconds.GetValueOrDefault(),
                "weekly reset is later than the 5-hour reset");
            AssertEqual(4, snapshot.AvailableResetCount.GetValueOrDefault(), "available reset count");
            AssertEqual(4, snapshot.AvailableResets.Count, "itemized available reset count");
            AssertEqual(
                "Full reset (Weekly + 5 hr)",
                snapshot.AvailableResets[0].Title,
                "reset title");
            AssertTrue(snapshot.AvailableResets[0].ExpiresAtUtc.HasValue, "reset expiration parses");
            AssertTrue(
                snapshot.AvailableResets[0].ExpiresAtUtc.Value.Kind == DateTimeKind.Utc,
                "reset expiration remains UTC");
            AssertEqual("pro", snapshot.PlanType, "plan type");
            AssertEqual(1, snapshot.AdditionalLimits.Count, "additional limit count");
            AssertEqual("GPT-5.3-Codex-Spark", snapshot.AdditionalLimits[0].DisplayName, "additional limit name");
            AssertEqual(100, snapshot.AdditionalLimits[0].FiveHour.RemainingPercent, "Spark 5-hour remaining");
            AssertEqual(100, snapshot.AdditionalLimits[0].Weekly.RemainingPercent, "Spark weekly remaining");
        }

        private static void TestMalformedResponses()
        {
            UsageSnapshot malformed = CodexRateLimitClient.ParseRateLimitsResponse("not-json");
            AssertTrue(!string.IsNullOrEmpty(malformed.ErrorMessage), "malformed JSON is rejected");

            const string invalidPercentage =
                "{\"result\":{\"rateLimits\":{\"primary\":{" +
                "\"usedPercent\":101,\"windowDurationMins\":300,\"resetsAt\":1783710153}}}}";
            UsageSnapshot invalid = CodexRateLimitClient.ParseRateLimitsResponse(invalidPercentage);
            AssertTrue(!string.IsNullOrEmpty(invalid.ErrorMessage), "out-of-range usage is rejected");
        }

        private static void TestPartialPrimaryLimits()
        {
            const string weeklyOnlyJson =
                "{\"result\":{\"rateLimits\":{\"primary\":{" +
                "\"usedPercent\":40,\"windowDurationMins\":10080}}}}";
            UsageSnapshot weeklyOnly = CodexRateLimitClient.ParseRateLimitsResponse(weeklyOnlyJson);
            AssertTrue(string.IsNullOrEmpty(weeklyOnly.ErrorMessage), "weekly-only usage is accepted");
            AssertTrue(weeklyOnly.Weekly != null, "weekly-only response keeps weekly usage");
            AssertTrue(weeklyOnly.FiveHour == null, "weekly-only response leaves 5-hour usage unavailable");

            const string fiveHourOnlyJson =
                "{\"result\":{\"rateLimits\":{\"primary\":{" +
                "\"usedPercent\":25,\"windowDurationMins\":300}}}}";
            UsageSnapshot fiveHourOnly = CodexRateLimitClient.ParseRateLimitsResponse(fiveHourOnlyJson);
            AssertTrue(string.IsNullOrEmpty(fiveHourOnly.ErrorMessage), "5-hour-only usage is accepted");
            AssertTrue(fiveHourOnly.FiveHour != null, "5-hour-only response keeps 5-hour usage");
            AssertTrue(fiveHourOnly.Weekly == null, "5-hour-only response leaves weekly usage unavailable");
        }

        private static void TestMissingPrimaryLimits()
        {
            const string auxiliaryOnlyJson =
                "{\"result\":{\"rateLimitsByLimitId\":{\"codex_spark\":{" +
                "\"limitName\":\"Spark\",\"primary\":{" +
                "\"usedPercent\":0,\"windowDurationMins\":300}}}}}";
            UsageSnapshot auxiliaryOnly = CodexRateLimitClient.ParseRateLimitsResponse(auxiliaryOnlyJson);
            AssertTrue(!string.IsNullOrEmpty(auxiliaryOnly.ErrorMessage), "auxiliary-only usage is rejected");
            AssertContains(
                auxiliaryOnly.ErrorMessage,
                "weekly or 5-hour",
                "missing primary limits have a specific error");
        }

        private static void TestRpcErrors()
        {
            const string rejectedJson =
                "{\"error\":{\"message\":\"  usage service\\nrejected\\trequest  \"}}";
            UsageSnapshot rejected = CodexRateLimitClient.ParseRateLimitsResponse(rejectedJson);
            AssertContains(
                rejected.ErrorMessage,
                "usage service rejected request",
                "RPC rejection preserves a compact reason");

            const string authenticationJson =
                "{\"error\":{\"message\":\"401 unauthorized: credential detail\"}}";
            UsageSnapshot authentication = CodexRateLimitClient.ParseRateLimitsResponse(authenticationJson);
            AssertEqual(
                "Codex is not signed in. Run codex login.",
                authentication.ErrorMessage,
                "authentication failures use safe guidance");

            string longReason = new string('x', 300);
            UsageSnapshot bounded = CodexRateLimitClient.ParseRateLimitsResponse(
                "{\"error\":{\"message\":\"" + longReason + "\"}}");
            AssertTrue(bounded.ErrorMessage.Length < 230, "RPC rejection reasons are length-limited");
        }

        private static void TestDeepClone()
        {
            UsageSnapshot original = new UsageSnapshot();
            original.FiveHour = new LimitWindow();
            original.FiveHour.UsedPercent = 25;
            UsageLimitSet additional = new UsageLimitSet();
            additional.DisplayName = "Example";
            additional.Weekly = new LimitWindow();
            additional.Weekly.UsedPercent = 50;
            original.AdditionalLimits.Add(additional);
            RateLimitResetCredit reset = new RateLimitResetCredit();
            reset.Title = "Original reset";
            reset.ExpiresAtUtc = new DateTime(2026, 7, 11, 18, 0, 0, DateTimeKind.Utc);
            original.AvailableResets.Add(reset);

            UsageSnapshot clone = original.Clone();
            clone.FiveHour.UsedPercent = 75;
            clone.AdditionalLimits[0].Weekly.UsedPercent = 80;
            clone.AvailableResets[0].Title = "Changed reset";

            AssertEqual(25, original.FiveHour.UsedPercent, "primary window clone is independent");
            AssertEqual(50, original.AdditionalLimits[0].Weekly.UsedPercent, "additional window clone is independent");
            AssertEqual("Original reset", original.AvailableResets[0].Title, "reset clone is independent");
        }

        private static void AssertTrue(bool condition, string name)
        {
            if (condition)
            {
                return;
            }

            failures++;
            Console.Error.WriteLine("FAIL: " + name);
        }

        private static void AssertEqual(int expected, int actual, string name)
        {
            if (expected == actual)
            {
                return;
            }

            failures++;
            Console.Error.WriteLine("FAIL: " + name + " (expected " + expected + ", actual " + actual + ")");
        }

        private static void AssertEqual(double expected, double actual, string name)
        {
            if (Math.Abs(expected - actual) < 0.0001)
            {
                return;
            }

            failures++;
            Console.Error.WriteLine("FAIL: " + name + " (expected " + expected + ", actual " + actual + ")");
        }

        private static void AssertEqual(string expected, string actual, string name)
        {
            if (string.Equals(expected, actual, StringComparison.Ordinal))
            {
                return;
            }

            failures++;
            Console.Error.WriteLine("FAIL: " + name + " (expected " + expected + ", actual " + actual + ")");
        }

        private static void AssertContains(string actual, string expectedFragment, string name)
        {
            if (!string.IsNullOrEmpty(actual) &&
                actual.IndexOf(expectedFragment, StringComparison.OrdinalIgnoreCase) >= 0)
            {
                return;
            }

            failures++;
            Console.Error.WriteLine(
                "FAIL: " + name + " (expected to find " + expectedFragment + ", actual " + actual + ")");
        }
    }
}
