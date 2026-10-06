using System;
using System.Collections.Generic;
using System.IO;

namespace CodexUsageTray.Tests
{
    internal static class UsageHistoryTests
    {
        public static void Run()
        {
            string directory = Path.Combine(Path.GetTempPath(), "CodexUsageHistoryTests-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(directory);
            try
            {
                TestPersistenceAndValidation(Path.Combine(directory, "history.json"));
                TestForecasts(Path.Combine(directory, "forecast.json"));
                TestProjections(Path.Combine(directory, "projection.json"));
                TestGapsAndResets(Path.Combine(directory, "cycles.json"));
                TestWeeklyDepletion(Path.Combine(directory, "weekly.json"));
                TestRecentPace(Path.Combine(directory, "recent.json"));
                TestDurationFormatting(Path.Combine(directory, "duration.json"));
                TestCorruptHistory(Path.Combine(directory, "corrupt.json"));
                TestHistoryImport(Path.Combine(directory, "import.json"));
                TestCycleAverage(Path.Combine(directory, "initial.json"));
                TestImportSaveFailure(Path.Combine(directory, "unwritable.json"));
                TestCycleRetention(Path.Combine(directory, "retention.json"));
                TestImportConsistency(Path.Combine(directory, "import-consistency.json"));
                UsageForecastEdgeCasesTests.Run();
            }
            finally
            {
                foreach (string file in Directory.GetFiles(directory)) File.Delete(file);
                Directory.Delete(directory);
            }
        }

        private static void TestPersistenceAndValidation(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            UsageSnapshot snapshot = Snapshot(now, 25, now.AddDays(3));
            Assert(store.Observe(snapshot), "first valid observation accepted");
            Assert(!store.Observe(snapshot), "duplicate timestamps rejected");
            Assert(!store.Observe(Snapshot(now.AddMinutes(-1), 20, now.AddDays(3))), "out-of-order sample rejected");
            snapshot.LastUpdated = now.AddMilliseconds(1);
            snapshot.IsRefreshing = true;
            Assert(!store.Observe(snapshot), "refresh-in-progress observation rejected");
            snapshot.IsRefreshing = false;
            snapshot.IsPaused = true;
            Assert(!store.Observe(snapshot), "paused observation rejected");
            Assert(store.GetForecast(false, snapshot, now) == "Refresh usage for an estimate", "paused snapshot suppresses the forecast");
            snapshot.IsPaused = false;
            snapshot.IsStale = true;
            Assert(!store.Observe(snapshot), "stale observation rejected");
            snapshot.IsStale = false;
            snapshot.ErrorMessage = "failed";
            Assert(!store.Observe(snapshot), "failed observation rejected");
            snapshot.ErrorMessage = null;
            snapshot.Weekly.UsedPercent = double.NaN;
            Assert(!store.Observe(snapshot), "nonfinite observation rejected");
            snapshot.Weekly.UsedPercent = 101;
            Assert(!store.Observe(snapshot), "out-of-range observation rejected");
            Assert(!store.Observe(Snapshot(now.AddDays(-29), 10, now)), "expired observation rejected");
            Assert(!store.Observe(Snapshot(now.AddMinutes(5), 10, now.AddDays(3))), "future observation rejected");
            UsageSnapshot extreme = Snapshot(DateTime.MaxValue, 10, null);
            extreme.Weekly.ResetAfterSeconds = int.MaxValue;
            Assert(!store.Observe(extreme), "extreme dates are rejected before deriving reset deadlines");

            UsageHistoryStore loaded = new UsageHistoryStore(path);
            Assert(loaded.Samples.Count == 1, "history survives restart");
            Assert(loaded.Samples[0].TimestampUtc.Kind == DateTimeKind.Utc, "persisted timestamps use UTC");
            Assert(loaded.Samples[0].Weekly.ResetAtUtc.Value.Kind == DateTimeKind.Utc, "persisted reset times use UTC");
            Assert(Math.Abs((loaded.Samples[0].Weekly.ResetAtUtc.Value - now.AddDays(3)).TotalSeconds) < 1,
                "reset deadline round-trips");
            List<UsageHistorySample> copy = loaded.Samples;
            copy[0].Weekly.UsedPercent = 99;
            copy.Clear();
            Assert(loaded.Samples[0].Weekly.UsedPercent == 25, "chart receives a defensive copy");
            loaded.Clear();
            Assert(new UsageHistoryStore(path).Samples.Count == 0, "clearing history persists");
        }

        private static void TestForecasts(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime reset = now.AddHours(4);
            UsageHistoryStore store = new UsageHistoryStore(path);
            UsageSnapshot latest = Snapshot(now, 50, reset);
            store.Observe(Snapshot(now.AddMinutes(-30), 25, reset));
            store.Observe(Snapshot(now.AddMinutes(-15), 37.5, reset));
            store.Observe(latest);
            Assert(store.GetForecast(false, latest, now) == "\u22481 hour left at current pace", "observed 50 percent per hour forecasts one hour");
            Assert(store.GetForecast(true, latest, now) == "Will last until reset", "cycle average is capped by the reset");
            Assert(store.GetForecast(false, latest, now.AddMinutes(31)) == "Refresh usage for an estimate", "old observations do not yield forecasts");

            store.Clear();
            reset = now.AddMinutes(30);
            store.Observe(Snapshot(now.AddMinutes(-30), 25, reset));
            store.Observe(Snapshot(now.AddMinutes(-15), 37.5, reset));
            latest = Snapshot(now, 50, reset);
            store.Observe(latest);
            Assert(store.GetForecast(false, latest, now) == "Will last until reset", "earlier reset replaces the depletion duration");
            latest.FiveHour.ResetAfterSeconds = 3600;
            Assert(store.GetForecast(false, latest, now) == "Will last until reset", "depletion exactly at reset lasts until reset");
            latest.FiveHour.ResetAfterSeconds = 3601;
            Assert(store.GetForecast(false, latest, now) == "\u22481 hour left at current pace", "depletion before reset retains its duration");

            store.Clear();
            reset = now.AddHours(4);
            store.Observe(Snapshot(now.AddMinutes(-30), 25, reset));
            store.Observe(Snapshot(now.AddMinutes(-15), 25, reset));
            latest = Snapshot(now, 25, reset);
            store.Observe(latest);
            Assert(store.GetForecast(false, latest, now) == "No usage increase in recent history", "idle observations do not invent exhaustion");
        }

        private static void TestProjections(string path)
        {
            DateTime reading = DateTime.UtcNow.AddMinutes(-10);
            DateTime queriedAt = reading.AddMinutes(5);
            DateTime reset = reading.AddHours(4);
            UsageHistoryStore store = new UsageHistoryStore(path);
            store.Observe(Snapshot(reading.AddMinutes(-30), 25, reset));
            store.Observe(Snapshot(reading.AddMinutes(-15), 37.5, reset));
            UsageSnapshot latest = Snapshot(reading, 50, reset);
            store.Observe(latest);

            UsageProjection projection = store.GetProjection(false, latest, queriedAt);
            Assert(projection != null && projection.StartUtc == reading && projection.EndUtc == reading.AddHours(1) &&
                projection.StartRemainingPercent == 50 && projection.EndRemainingPercent == 0 && !projection.EndsAtReset,
                "observed pace starts at the actual reading and stops at depletion before reset");
            latest.FiveHour.ResetAfterSeconds = null;
            Assert(store.GetProjection(false, latest, queriedAt) == null,
                "observed pace without a known reset deadline has no projection");

            latest.FiveHour.ResetAfterSeconds = 1800;
            projection = store.GetProjection(false, latest, queriedAt);
            Assert(projection != null && projection.EndUtc == reading.AddMinutes(30) &&
                projection.EndRemainingPercent == 25 && projection.EndsAtReset,
                "projection stops at an earlier reset with positive remaining usage");
            latest.FiveHour.ResetAfterSeconds = 3600;
            projection = store.GetProjection(false, latest, queriedAt);
            Assert(projection != null && projection.EndUtc == reading.AddHours(1) &&
                projection.EndRemainingPercent == 0 && projection.EndsAtReset,
                "depletion exactly at reset is marked as ending at reset");
            latest.FiveHour.ResetAfterSeconds = 3601;
            projection = store.GetProjection(false, latest, queriedAt);
            Assert(projection != null && projection.EndUtc == reading.AddHours(1) && !projection.EndsAtReset &&
                store.GetForecast(false, latest, queriedAt) == "\u22481 hour left at current pace",
                "forecast and projection agree for a fresh reading taken before the query");

            store.Clear();
            store.Observe(Snapshot(reading.AddMinutes(-30), 25, reset));
            store.Observe(Snapshot(reading.AddMinutes(-15), 25, reset));
            latest = Snapshot(reading, 25, reset);
            store.Observe(latest);
            projection = store.GetProjection(false, latest, queriedAt);
            Assert(projection != null && projection.EndUtc == reset &&
                projection.StartRemainingPercent == 75 && projection.EndRemainingPercent == 75 && projection.EndsAtReset,
                "known zero recent consumption stays flat through reset");

            store.Clear();
            store.Observe(Snapshot(reading.AddMinutes(-30), 25.0005, reset));
            store.Observe(Snapshot(reading.AddMinutes(-15), 25.00025, reset));
            latest = Snapshot(reading, 25, reset);
            store.Observe(latest);
            projection = store.GetProjection(false, latest, queriedAt);
            Assert(projection != null && projection.EndUtc == reset &&
                projection.EndRemainingPercent == 75 && projection.EndsAtReset,
                "tiny measurement declines do not increase projected remaining usage");

            store.Clear();
            latest = Snapshot(reading, 20, reading.AddDays(6));
            projection = store.GetProjection(true, latest, queriedAt);
            Assert(projection != null && projection.StartUtc == reading && projection.EndUtc == reading.AddDays(4) &&
                projection.StartRemainingPercent == 80 && projection.EndRemainingPercent == 0 && !projection.EndsAtReset,
                "valid cycle average supplies a projection before history is sufficient");
            latest.Weekly.UsedPercent = 10;
            projection = store.GetProjection(true, latest, queriedAt);
            Assert(projection != null && projection.EndUtc == reading.AddDays(6) &&
                projection.EndRemainingPercent == 30 && projection.EndsAtReset,
                "cycle average projection retains usage at reset");

            latest.Weekly.ResetAfterSeconds = null;
            Assert(store.GetProjection(true, latest, queriedAt) == null, "unknown reset deadline has no projection");
            latest.Weekly.ResetAfterSeconds = 7 * 86400 - 30;
            Assert(store.GetProjection(true, latest, queriedAt) == null, "insufficient rate information has no projection");
            latest = Snapshot(reading, 20, reading.AddDays(6));
            latest.IsRefreshing = true;
            Assert(store.GetProjection(true, latest, queriedAt) != null, "refresh in progress retains the last valid projection");
            latest.IsRefreshing = false;
            latest.IsPaused = true;
            Assert(store.GetProjection(true, latest, queriedAt) == null, "paused data suppresses projection");
            latest.IsPaused = false;
            latest.IsStale = true;
            Assert(store.GetProjection(true, latest, queriedAt) == null, "stale data suppresses projection");
            latest.IsStale = false;
            latest.ErrorMessage = "failed";
            Assert(store.GetProjection(true, latest, queriedAt) == null, "failed data suppresses projection");
            latest.ErrorMessage = null;
            latest.Weekly.UsedPercent = 100;
            Assert(store.GetProjection(true, latest, queriedAt) == null, "exhausted usage has no projection");
            latest.Weekly.UsedPercent = 20;
            Assert(store.GetProjection(true, latest, reading.AddMinutes(31)) == null,
                "expired reading has no projection");
            latest.Weekly.ResetAfterSeconds = 60;
            Assert(store.GetProjection(true, latest, queriedAt) == null,
                "expired reset deadline has no projection");
        }

        private static void TestGapsAndResets(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime reset = now.AddDays(3);
            UsageHistoryStore store = new UsageHistoryStore(path);
            store.Observe(Snapshot(now.AddHours(-24), 10, reset));
            store.Observe(Snapshot(now.AddHours(-12), 30, reset));
            UsageSnapshot latest = Snapshot(now, 50, reset);
            store.Observe(latest);
            Assert(store.GetForecast(true, latest, now) == "\u22481.3 days (30h 0m) left at current pace", "weekly wall-clock rate bridges overnight observations in a known cycle");
            List<UsageHistorySample> samples = store.Samples;
            Assert(!UsageHistoryStore.IsContinuous(samples[0], samples[1], true), "overnight chart gaps remain visible");
            Assert(!UsageHistoryStore.IsReset(samples[0], samples[1], true), "a chart gap is not a reset");

            store.Clear();
            store.Observe(Snapshot(now.AddHours(-24), 10, null));
            store.Observe(Snapshot(now.AddHours(-12), 30, null));
            latest = Snapshot(now, 50, null);
            store.Observe(latest);
            Assert(store.GetForecast(true, latest, now) == "Learning usage pace...", "unknown cycles cannot bridge overnight gaps");

            store.Clear();
            store.Observe(Snapshot(now.AddMinutes(-30), 90, reset));
            UsageHistorySample beforeReset = store.Samples[0];
            store.Observe(Snapshot(now.AddMinutes(-15), 10, reset));
            latest = Snapshot(now, 20, reset);
            store.Observe(latest);
            samples = store.Samples;
            Assert(UsageHistoryStore.IsReset(beforeReset, samples[1], false), "banked reset detected from a decrease");
            Assert(!UsageHistoryStore.IsContinuous(beforeReset, samples[1], false), "chart does not connect across banked reset");
            Assert(samples.Count == 3 && samples[1].FiveHour.UsedPercent == 10,
                "earlier cycles remain available for pattern learning");
            Assert(store.GetForecast(false, latest, now) == "Learning usage pace...", "forecast cannot reuse consumption before reset");
            samples[1].FiveHour.UsedPercent = 95;
            samples[1].FiveHour.ResetAtUtc = reset.AddHours(5);
            Assert(UsageHistoryStore.IsReset(beforeReset, samples[1], false), "changed reset deadline detects new cycle even when usage increased");
        }

        private static void TestWeeklyDepletion(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            DateTime reset = now.AddDays(6);
            store.Observe(Snapshot(now.AddDays(-1), 0, reset));
            store.Observe(Snapshot(now.AddHours(-12), 10, reset));
            UsageSnapshot latest = Snapshot(now, 20, reset);
            store.Observe(latest);
            Assert(store.GetForecast(true, latest, now) == "\u22484 days (96h 0m) left at current pace", "80 percent remaining at 20 percent per day lasts four days");
            Assert(store.GetForecast(true, latest, now.AddHours(1)) == "Refresh usage for an estimate", "stale observations cannot project depletion");
            latest.Weekly.ResetAfterSeconds = null;
            Assert(store.GetForecast(true, latest, now) == "\u22484 days (96h 0m) left at current pace", "depletion uses observed consumption rather than time until reset");
            latest.Weekly.ResetAfterSeconds = 2 * 86400;
            Assert(store.GetForecast(true, latest, now) == "Will last until reset", "weekly estimate is capped by its actual reset deadline");
            latest.Weekly.ResetAfterSeconds = 0;
            Assert(store.GetForecast(true, latest, now) == "Awaiting reset update", "expired reset needs a fresh reading");
        }

        private static void TestRecentPace(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime reset = now.AddDays(3);
            UsageHistoryStore store = new UsageHistoryStore(path);
            store.Observe(Snapshot(now.AddHours(-12), 0, reset));
            store.Observe(Snapshot(now.AddHours(-6), 6, reset));
            UsageSnapshot latest = Snapshot(now, 30, reset);
            store.Observe(latest);
            UsageProjection projection = store.GetProjection(true, latest, now);
            Assert(projection != null && Math.Abs((projection.EndUtc - now).TotalHours - 70.0 / 3) < 0.001,
                "weekly acceleration favors recent consumption over the old 28-hour estimate");
            Assert(store.GetForecast(true, latest, now) == "\u224823 hours 20 minutes left at current pace",
                "forecast text uses the same recent weighted rate as the projection");

            store.Clear();
            store.Observe(Snapshot(now.AddHours(-12), 0, reset));
            store.Observe(Snapshot(now.AddHours(-6), 24, reset));
            store.Observe(latest);
            projection = store.GetProjection(true, latest, now);
            Assert(projection != null && Math.Abs((projection.EndUtc - now).TotalHours - 35) < 0.001,
                "weekly slowdown extends the estimate instead of always biasing toward exhaustion");

            store.Clear();
            store.Observe(Snapshot(now.AddHours(-12), 0, reset));
            store.Observe(Snapshot(now.AddHours(-6), 6, reset));
            store.Observe(Snapshot(now.AddHours(-5), 10, reset));
            store.Observe(Snapshot(now.AddHours(-1), 26, reset));
            store.Observe(latest);
            projection = store.GetProjection(true, latest, now);
            Assert(projection != null && Math.Abs((projection.EndUtc - now).TotalHours - 70.0 / 3) < 0.001,
                "additional irregular readings along the same trend do not change the estimate");

            store.Clear();
            reset = now.AddHours(4);
            store.Observe(Snapshot(now.AddHours(-1), 0, reset));
            store.Observe(Snapshot(now.AddMinutes(-30), 6, reset));
            latest = Snapshot(now, 30, reset);
            store.Observe(latest);
            projection = store.GetProjection(false, latest, now);
            Assert(projection != null && Math.Abs((projection.EndUtc - now).TotalHours - 70.0 / 40.8) < 0.001,
                "five-hour acceleration responds to the last half-hour of usage");

            store.Clear();
            store.Observe(Snapshot(now.AddHours(-1), 0, reset));
            store.Observe(Snapshot(now.AddMinutes(-30), 30, reset));
            store.Observe(latest);
            projection = store.GetProjection(false, latest, now);
            Assert(projection != null && projection.EndsAtReset &&
                Math.Abs(projection.EndRemainingPercent - 22) < 0.001,
                "recent idle time slows consumption instead of being discarded");
        }

        private static void TestDurationFormatting(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            foreach (double duration in new[] { 55.2, 55.35, 1.75 })
            {
                store.Clear();
                bool weekly = duration >= 24;
                double span = weekly ? 12 : 0.5;
                double rate = 50 / duration;
                DateTime reset = weekly ? now.AddDays(3) : now.AddHours(4);
                store.Observe(Snapshot(now.AddHours(-span), 50 - rate * span, reset));
                store.Observe(Snapshot(now.AddHours(-span / 2), 50 - rate * span / 2, reset));
                UsageSnapshot latest = Snapshot(now, 50, reset);
                store.Observe(latest);
                string expected = duration == 55.2 ? "\u22482.3 days (55h 12m) left at current pace" :
                    duration == 55.35 ? "\u22482.3 days (55h 21m) left at current pace" :
                    "\u22481 hour 45 minutes left at current pace";
                Assert(store.GetForecast(weekly, latest, now) == expected,
                    "duration retains decimal days alongside hours and minutes from the full calculation");
                UsageProjection projection = store.GetProjection(weekly, latest, now);
                Assert(projection != null && Math.Abs((projection.EndUtc - now).TotalHours - duration) < 0.001,
                    "duration formatting retains the precise projected depletion time");
            }
        }

        private static void TestCycleRetention(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime oldReset = now.AddHours(-3);
            DateTime reset = oldReset.AddDays(7);
            UsageHistoryStore store = new UsageHistoryStore(path);
            store.Observe(Snapshot(now.AddDays(-1), 50, oldReset));
            store.Observe(Snapshot(now.AddHours(-4), 90, oldReset));
            store.Observe(Snapshot(now.AddHours(-2), 0, reset));
            store.Observe(Snapshot(now.AddHours(-1), 10, reset));
            Assert(store.Samples.Count == 4 && store.Samples[0].TimestampUtc == now.AddDays(-1),
                "prior and current cycles remain in memory for learning");
            store = new UsageHistoryStore(path);
            Assert(store.Samples.Count == 4 && File.ReadAllText(path).Contains("UsedPercent\":90"),
                "prior cycles survive in the bounded saved history");
            UsageSnapshot latest = Snapshot(now, 10, reset);
            store.Observe(latest);
            store = new UsageHistoryStore(path);
            Assert(store.Samples.Count == 5, "later idle readings preserve both cycles after restart");

            store.Clear();
            store.Observe(Snapshot(now.AddHours(-2), 90, reset));
            store.Observe(Snapshot(now.AddMinutes(-30), 10, reset));
            store = new UsageHistoryStore(path);
            latest = Snapshot(now, 15, reset);
            store.Observe(latest);
            Assert(store.GetForecast(true, latest, now) == "Learning usage pace...",
                "restart preserves banked-reset evidence so a cycle average cannot reuse the old start");
            File.WriteAllText(path + ".imported", "1");
            store = new UsageHistoryStore(path);
            Assert(!store.ImportCompleted, "older import markers allow prior-cycle backfill after upgrading");
            store.Import(new UsageSnapshot[0]);
            Assert(new UsageHistoryStore(path).ImportCompleted, "updated import version persists");
        }

        private static void TestCorruptHistory(string path)
        {
            File.WriteAllText(path, "{invalid history");
            UsageHistoryStore store = new UsageHistoryStore(path);
            Assert(store.Samples.Count == 0, "corrupt history starts empty");
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            Assert(store.Observe(Snapshot(now, 10, now.AddDays(1))), "new observations recover corrupt history");
            Assert(new UsageHistoryStore(path).Samples.Count == 1, "recovered history persists");
        }

        private static void TestImportConsistency(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime reset = now.AddDays(3);
            UsageHistoryStore store = new UsageHistoryStore(path);
            store.Observe(Snapshot(now.AddHours(-1), 20, reset));
            UsageSnapshot current = Snapshot(now, 25, reset);
            store.Observe(current);
            store.Import(new[]
            {
                Snapshot(now.AddMinutes(-50), 19, reset),
                Snapshot(now.AddMinutes(-45), 21, reset),
                Snapshot(now.AddMinutes(-40), 20, reset),
                Snapshot(now.AddMinutes(-30), 99, reset)
            }, false, false);
            Assert(store.Samples.Count == 3 && store.Samples[1].Weekly.UsedPercent == 21,
                "cached lower quota and quota contradicting a live reading do not invent resets");
            Assert(!store.ImportCompleted && !File.Exists(path + ".imported"),
                "an interrupted import remains eligible to resume");
            store.Import(new UsageSnapshot[0]);
            store = new UsageHistoryStore(path);
            Assert(store.ImportCompleted && store.Samples[1].Weekly.IsImported,
                "import completion and provenance survive restart");
            Assert(store.Import(new[] { Snapshot(now.AddMinutes(-20), 22, reset) }) == 0,
                "completed backfill is not repeated");

            store.Clear();
            store.Observe(Snapshot(now.AddDays(-21), 10, now.AddDays(-16)));
            store.Observe(Snapshot(now.AddDays(-7), 10, now.AddDays(-2)));
            store.Observe(Snapshot(now, 10, reset));
            Assert(new UsageHistoryStore(path).Samples.Count == 3,
                "four-week learning retains readings older than two weeks");
        }

        private static void TestHistoryImport(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-2);
            DateTime reset = now.AddDays(6);
            UsageHistoryStore store = new UsageHistoryStore(path);
            UsageSnapshot current = Snapshot(now, 20, reset);
            store.Observe(current);
            UsageSnapshot unrelatedWindow = Snapshot(now.AddHours(-6), 15, reset);
            unrelatedWindow.Weekly = null;
            List<UsageSnapshot> imported = new List<UsageSnapshot>
            {
                Snapshot(now.AddHours(-12), 10, reset),
                Snapshot(now.AddDays(-1), 0, reset),
                Snapshot(now, 99, reset),
                unrelatedWindow
            };
            Assert(store.Import(imported) == 3, "earlier observations are merged and duplicate live timestamp is preserved");
            Assert(store.Samples[3].Weekly.UsedPercent == 20, "import cannot replace a live reading");
            Assert(store.GetForecast(true, current, now) == "\u22484 days (96h 0m) left at current pace",
                "backfilled data forecasts immediately across partial-window observations");
            UsageHistoryStore loaded = new UsageHistoryStore(path);
            Assert(loaded.ImportCompleted && loaded.Samples.Count == 4, "import and completion survive restart");
            loaded.Clear();
            UsageHistoryStore cleared = new UsageHistoryStore(path);
            Assert(cleared.ImportCompleted && cleared.Import(imported) == 0 && cleared.Samples.Count == 0,
                "cleared history is not silently reimported");
        }

        private static void TestCycleAverage(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            UsageSnapshot snapshot = Snapshot(now, 20, now.AddDays(6));
            Assert(store.GetForecast(true, snapshot, now) == "\u22484 days (96h 0m) left at cycle average",
                "first reading of 20 percent in one day estimates four days without learning");
            snapshot.Weekly.UsedPercent = 10;
            Assert(store.GetForecast(true, snapshot, now) == "Will last until reset",
                "initial weekly estimate beyond reset uses the survival label");
            snapshot.FiveHour.UsedPercent = 20;
            snapshot.FiveHour.ResetAfterSeconds = 4 * 3600;
            Assert(store.GetForecast(false, snapshot, now) == "Will last until reset",
                "initial five-hour estimate exactly at reset uses the survival label");
            snapshot.Weekly.UsedPercent = 0;
            Assert(store.GetForecast(true, snapshot, now) == "No usage yet this cycle", "zero consumption has no invented depletion");
            snapshot.Weekly.UsedPercent = 20;
            snapshot.Weekly.ResetAfterSeconds = 7 * 86400 - 30;
            Assert(store.GetForecast(true, snapshot, now) == "Learning usage pace...", "first moments of the cycle are not projected");
            snapshot.Weekly.ResetAfterSeconds = null;
            Assert(store.GetForecast(true, snapshot, now) == "Learning usage pace...", "unknown cycle start needs observations");
            snapshot = Snapshot(now, 20, now.AddDays(6));
            store.Observe(Snapshot(now.AddHours(-2), 60, now.AddDays(6)));
            store.Observe(Snapshot(now.AddHours(-1), 10, now.AddDays(6)));
            store.Observe(snapshot);
            Assert(store.GetForecast(true, snapshot, now) == "Learning usage pace...", "observed banked reset blocks a misleading cycle average");
        }

        private static void TestImportSaveFailure(string path)
        {
            Directory.CreateDirectory(path);
            try
            {
                DateTime now = DateTime.UtcNow.AddSeconds(-1);
                UsageHistoryStore store = new UsageHistoryStore(path);
                store.Import(new[] { Snapshot(now, 20, now.AddDays(6)) });
                Assert(!File.Exists(path + ".imported") && !new UsageHistoryStore(path).ImportCompleted,
                    "failed history save must allow bootstrap retry after restart");
            }
            finally { Directory.Delete(path); }
        }

        private static UsageSnapshot Snapshot(DateTime timestamp, double used, DateTime? reset)
        {
            int? seconds = reset.HasValue ? (int?)(int)(reset.Value - timestamp).TotalSeconds : null;
            return new UsageSnapshot
            {
                LastUpdated = timestamp,
                LastAttempted = timestamp,
                Weekly = new LimitWindow { UsedPercent = used, WindowMinutes = 10080, ResetAfterSeconds = seconds },
                FiveHour = new LimitWindow { UsedPercent = used, WindowMinutes = 300, ResetAfterSeconds = seconds }
            };
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("Usage history: " + message);
        }
    }
}
