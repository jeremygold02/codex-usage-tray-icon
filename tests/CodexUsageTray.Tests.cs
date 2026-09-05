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
            TestResetCreditRedemptionResponses();
            TestDeepClone();
            TestUsageResetDetection();
            TestBankedResetDetection();
            TestUsageResetNotificationSuppression();
            TestAutomaticResetRedemptionPolicy();

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
            AssertEqual("credit-1", snapshot.AvailableResets[0].Id, "reset credit ID");
            AssertEqual(
                "codexRateLimits",
                snapshot.AvailableResets[0].ResetType,
                "reset credit type");
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
            reset.Id = "original-id";
            reset.ResetType = "codexRateLimits";
            reset.Title = "Original reset";
            reset.ExpiresAtUtc = new DateTime(2026, 7, 11, 18, 0, 0, DateTimeKind.Utc);
            original.AvailableResets.Add(reset);

            UsageSnapshot clone = original.Clone();
            clone.FiveHour.UsedPercent = 75;
            clone.AdditionalLimits[0].Weekly.UsedPercent = 80;
            clone.AvailableResets[0].Id = "changed-id";
            clone.AvailableResets[0].Title = "Changed reset";

            AssertEqual(25, original.FiveHour.UsedPercent, "primary window clone is independent");
            AssertEqual(50, original.AdditionalLimits[0].Weekly.UsedPercent, "additional window clone is independent");
            AssertEqual("original-id", original.AvailableResets[0].Id, "reset ID clone is independent");
            AssertEqual("Original reset", original.AvailableResets[0].Title, "reset clone is independent");
        }

        private static void TestResetCreditRedemptionResponses()
        {
            AssertRedemptionOutcome(
                ResetCreditRedemptionOutcome.Reset,
                CodexRateLimitClient.ParseResetCreditRedemptionResponse(
                    "{\"result\":{\"outcome\":\"reset\"}}"),
                "reset redemption outcome");
            AssertRedemptionOutcome(
                ResetCreditRedemptionOutcome.AlreadyRedeemed,
                CodexRateLimitClient.ParseResetCreditRedemptionResponse(
                    "{\"result\":{\"outcome\":\"alreadyRedeemed\"}}"),
                "already-redeemed outcome");
            AssertRedemptionOutcome(
                ResetCreditRedemptionOutcome.NothingToReset,
                CodexRateLimitClient.ParseResetCreditRedemptionResponse(
                    "{\"result\":{\"outcome\":\"nothingToReset\"}}"),
                "nothing-to-reset outcome");
            AssertRedemptionOutcome(
                ResetCreditRedemptionOutcome.NoCredit,
                CodexRateLimitClient.ParseResetCreditRedemptionResponse(
                    "{\"result\":{\"outcome\":\"noCredit\"}}"),
                "no-credit outcome");

            ResetCreditRedemptionResult authentication =
                CodexRateLimitClient.ParseResetCreditRedemptionResponse(
                    "{\"error\":{\"message\":\"401 unauthorized\"}}");
            AssertRedemptionOutcome(
                ResetCreditRedemptionOutcome.Failed,
                authentication,
                "redemption authentication failure");
            AssertContains(
                authentication.ErrorMessage,
                "codex login",
                "redemption authentication guidance");

            ResetCreditRedemptionResult unknown =
                CodexRateLimitClient.ParseResetCreditRedemptionResponse(
                    "{\"result\":{\"outcome\":\"futureOutcome\"}}");
            AssertRedemptionOutcome(
                ResetCreditRedemptionOutcome.Failed,
                unknown,
                "unknown redemption outcome");
        }

        private static void TestUsageResetDetection()
        {
            UsageResetDetector startupFull = new UsageResetDetector();
            AssertReset(
                UsageResetKind.None,
                startupFull.Observe(CreateSnapshot(100, 100)),
                "startup at full usage is silent");
            AssertReset(
                UsageResetKind.None,
                startupFull.Observe(CreateSnapshot(100, 100)),
                "repeated full usage is silent");
            startupFull.Observe(CreateSnapshot(99, 100));
            AssertReset(
                UsageResetKind.None,
                startupFull.Observe(CreateSnapshot(100, 100)),
                "one startup-cycle 99 percent sample is treated as jitter");

            UsageResetDetector independentWindows = new UsageResetDetector();
            AssertReset(
                UsageResetKind.None,
                independentWindows.Observe(CreateSnapshot(72, 45)),
                "initial below-full usage establishes a baseline");
            AssertReset(
                UsageResetKind.Weekly,
                independentWindows.Observe(CreateSnapshot(100, 40)),
                "weekly reset is detected independently");
            AssertReset(
                UsageResetKind.None,
                independentWindows.Observe(CreateSnapshot(100, 40)),
                "weekly reset notification is not duplicated");
            AssertReset(
                UsageResetKind.FiveHour,
                independentWindows.Observe(CreateSnapshot(100, 100)),
                "5-hour reset is detected independently");

            UsageResetDetector combinedReset = new UsageResetDetector();
            combinedReset.Observe(CreateSnapshot(60, 70));
            AssertReset(
                UsageResetKind.Weekly | UsageResetKind.FiveHour,
                combinedReset.Observe(CreateSnapshot(100, 100)),
                "simultaneous resets are combined");

            UsageResetDetector missingWindow = new UsageResetDetector();
            missingWindow.Observe(CreateSnapshot(80, null));
            missingWindow.Observe(CreateSnapshot(null, null));
            AssertReset(
                UsageResetKind.Weekly,
                missingWindow.Observe(CreateSnapshot(100, null)),
                "missing windows preserve reset state");

            UsageResetDetector jitterSuppression = new UsageResetDetector();
            jitterSuppression.Observe(CreateSnapshot(50, null));
            AssertReset(
                UsageResetKind.Weekly,
                jitterSuppression.Observe(CreateSnapshot(100, null)),
                "first reset is detected");
            AssertReset(
                UsageResetKind.None,
                jitterSuppression.Observe(CreateSnapshot(99, null)),
                "one 99 percent sample does not immediately rearm");
            AssertReset(
                UsageResetKind.None,
                jitterSuppression.Observe(CreateSnapshot(100, null)),
                "99 to 100 jitter does not duplicate a notification");
            jitterSuppression.Observe(CreateSnapshot(99, null));
            jitterSuppression.Observe(CreateSnapshot(99, null));
            AssertReset(
                UsageResetKind.Weekly,
                jitterSuppression.Observe(CreateSnapshot(100, null)),
                "stable below-full usage rearms detection");

            UsageResetDetector immediateRearm = new UsageResetDetector();
            immediateRearm.Observe(CreateSnapshot(50, null));
            immediateRearm.Observe(CreateSnapshot(100, null));
            immediateRearm.Observe(CreateSnapshot(98, null));
            AssertReset(
                UsageResetKind.Weekly,
                immediateRearm.Observe(CreateSnapshot(100, null)),
                "98 percent usage rearms detection immediately");
        }

        private static void TestUsageResetNotificationSuppression()
        {
            DateTime startedAtUtc =
                new DateTime(2026, 7, 26, 23, 42, 17, DateTimeKind.Utc);
            UsageResetNotificationSuppression suppression =
                new UsageResetNotificationSuppression();

            suppression.SuppressFor(startedAtUtc, TimeSpan.FromMinutes(30));
            AssertReset(
                UsageResetKind.None,
                suppression.Filter(
                    UsageResetKind.None,
                    startedAtUtc.AddMinutes(1)),
                "post-redemption refresh keeps reset notifications suppressed");
            AssertReset(
                UsageResetKind.None,
                suppression.Filter(
                    UsageResetKind.Weekly,
                    startedAtUtc.AddMinutes(2)),
                "delayed weekly reset notification is suppressed");
            AssertReset(
                UsageResetKind.None,
                suppression.Filter(
                    UsageResetKind.FiveHour,
                    startedAtUtc.AddMinutes(3)),
                "delayed 5-hour reset notification is suppressed");
            AssertReset(
                UsageResetKind.Weekly,
                suppression.Filter(
                    UsageResetKind.Weekly,
                    startedAtUtc.AddMinutes(31)),
                "reset notifications resume after suppression expires");
        }

        private static void TestBankedResetDetection()
        {
            BankedResetDetector detector = new BankedResetDetector();
            UsageSnapshot initial = CreateSnapshot(80, 70);
            initial.AvailableResetCount = 2;
            initial.AvailableResets.Add(CreateCredit("credit-1", "codexRateLimits", DateTime.UtcNow));
            initial.AvailableResets.Add(CreateCredit("credit-2", "codexRateLimits", DateTime.UtcNow));
            AssertEqual(0, detector.Observe(initial), "initial banked resets establish a baseline");

            UsageSnapshot added = initial.Clone();
            added.AvailableResetCount = 3;
            added.AvailableResets.Add(CreateCredit("credit-3", "codexRateLimits", DateTime.UtcNow));
            AssertEqual(1, detector.Observe(added), "a higher banked reset count is detected");
            AssertEqual(3, detector.CurrentAvailableCount, "current banked reset count is retained");
            AssertEqual(0, detector.Observe(added), "unchanged banked resets are not duplicated");

            UsageSnapshot replacement = added.Clone();
            replacement.AvailableResets.RemoveAt(0);
            replacement.AvailableResets.Add(
                CreateCredit("credit-4", "codexRateLimits", DateTime.UtcNow));
            AssertEqual(
                1,
                detector.Observe(replacement),
                "a new itemized reset is detected when the total is unchanged");

            UsageSnapshot missing = CreateSnapshot(80, 70);
            AssertEqual(0, detector.Observe(missing), "missing reset data preserves detector state");
            AssertEqual(3, detector.CurrentAvailableCount, "missing reset data preserves the last count");

            UsageSnapshot none = CreateSnapshot(80, 70);
            none.AvailableResetCount = 0;
            AssertEqual(0, detector.Observe(none), "using resets does not produce a notification");
            AssertEqual(0, detector.CurrentAvailableCount, "zero available resets updates the count");

            UsageSnapshot replenished = CreateSnapshot(80, 70);
            replenished.AvailableResetCount = 1;
            AssertEqual(1, detector.Observe(replenished), "a later banked reset is detected");

            BankedResetDetector restarted = new BankedResetDetector(detector.CreateState());
            UsageSnapshot afterRestart = replenished.Clone();
            afterRestart.AvailableResetCount = 2;
            AssertEqual(
                1,
                restarted.Observe(afterRestart),
                "a banked reset added while the app is closed is detected after restart");

            string cachePath = Path.Combine(
                Path.GetTempPath(),
                "CodexUsageTray-banked-resets-" + Guid.NewGuid().ToString("N") + ".json");
            try
            {
                BankedResetStateStore store = new BankedResetStateStore(cachePath);
                store.Save(restarted.CreateState());
                BankedResetDetector loaded = new BankedResetDetector(store.Load());
                AssertEqual(
                    0,
                    loaded.Observe(afterRestart),
                    "persisted banked reset state prevents duplicate startup notifications");
            }
            finally
            {
                if (File.Exists(cachePath))
                {
                    File.Delete(cachePath);
                }
            }
        }

        private static void TestAutomaticResetRedemptionPolicy()
        {
            DateTime nowUtc = new DateTime(2026, 7, 26, 16, 0, 0, DateTimeKind.Utc);
            UsageSnapshot snapshot = CreateSnapshot(75, 100);
            snapshot.AvailableResets.Add(
                CreateCredit("unsupported", "otherReset", nowUtc.AddMinutes(1)));
            snapshot.AvailableResets.Add(
                CreateCredit("outside-window", "codexRateLimits", nowUtc.AddMinutes(6)));
            snapshot.AvailableResets.Add(
                CreateCredit("eligible", "codex_rate_limits", nowUtc.AddMinutes(4)));

            RateLimitResetCredit selected =
                AutomaticResetRedemptionPolicy.FindEligibleCredit(
                    snapshot,
                    nowUtc,
                    5,
                    null);
            AssertTrue(selected != null, "eligible expiring reset is selected");
            AssertEqual("eligible", selected != null ? selected.Id : null, "earliest eligible reset");

            RateLimitResetCredit excluded =
                AutomaticResetRedemptionPolicy.FindEligibleCredit(
                    snapshot,
                    nowUtc,
                    5,
                    "eligible");
            AssertTrue(excluded == null, "completed reset is excluded");

            UsageSnapshot fullUsage = CreateSnapshot(100, 100);
            fullUsage.AvailableResets.Add(
                CreateCredit("unused", "codexRateLimits", nowUtc.AddMinutes(1)));
            AssertTrue(
                AutomaticResetRedemptionPolicy.FindEligibleCredit(
                    fullUsage,
                    nowUtc,
                    5,
                    null) == null,
                "full usage does not consume a reset");

            DateTime? nextCheckUtc =
                AutomaticResetRedemptionPolicy.GetNextCheckUtc(
                    snapshot,
                    nowUtc,
                    5,
                    "eligible");
            AssertTrue(nextCheckUtc.HasValue, "future reset schedules an expiry check");
            AssertTrue(
                nextCheckUtc.GetValueOrDefault() == nowUtc.AddMinutes(1),
                "expiry check uses the configured lead time");
        }

        private static UsageSnapshot CreateSnapshot(int? weeklyRemaining, int? fiveHourRemaining)
        {
            UsageSnapshot snapshot = new UsageSnapshot();
            snapshot.Weekly = CreateWindow(weeklyRemaining);
            snapshot.FiveHour = CreateWindow(fiveHourRemaining);
            return snapshot;
        }

        private static LimitWindow CreateWindow(int? remaining)
        {
            if (!remaining.HasValue)
            {
                return null;
            }

            LimitWindow window = new LimitWindow();
            window.UsedPercent = 100 - remaining.Value;
            return window;
        }

        private static RateLimitResetCredit CreateCredit(
            string id,
            string resetType,
            DateTime expiresAtUtc)
        {
            RateLimitResetCredit credit = new RateLimitResetCredit();
            credit.Id = id;
            credit.ResetType = resetType;
            credit.ExpiresAtUtc = expiresAtUtc;
            return credit;
        }

        private static void AssertReset(UsageResetKind expected, UsageResetKind actual, string name)
        {
            AssertEqual((int)expected, (int)actual, name);
        }

        private static void AssertRedemptionOutcome(
            ResetCreditRedemptionOutcome expected,
            ResetCreditRedemptionResult actual,
            string name)
        {
            AssertTrue(actual != null, name + " returns a result");
            if (actual != null)
            {
                AssertEqual((int)expected, (int)actual.Outcome, name);
            }
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
