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
                TestGapsAndResets(Path.Combine(directory, "cycles.json"));
                TestWeeklyDepletion(Path.Combine(directory, "weekly.json"));
                TestCorruptHistory(Path.Combine(directory, "corrupt.json"));
                TestHistoryImport(Path.Combine(directory, "import.json"));
                TestCycleAverage(Path.Combine(directory, "initial.json"));
                TestImportSaveFailure(Path.Combine(directory, "unwritable.json"));
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
            Assert(!store.Observe(Snapshot(now.AddDays(-15), 10, now)), "expired observation rejected");
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
            Assert(store.GetForecast(true, latest, now).Contains("at cycle average"), "short observation history falls back to the cycle average");
            Assert(store.GetForecast(false, latest, now.AddMinutes(31)) == "Refresh usage for an estimate", "old observations do not yield forecasts");

            store.Clear();
            reset = now.AddMinutes(30);
            store.Observe(Snapshot(now.AddMinutes(-30), 25, reset));
            store.Observe(Snapshot(now.AddMinutes(-15), 37.5, reset));
            latest = Snapshot(now, 50, reset);
            store.Observe(latest);
            Assert(store.GetForecast(false, latest, now) == "\u22481 hour left at current pace (resets sooner)", "earlier reset preserves the consumption-based duration");

            store.Clear();
            reset = now.AddHours(4);
            store.Observe(Snapshot(now.AddMinutes(-30), 25, reset));
            store.Observe(Snapshot(now.AddMinutes(-15), 25, reset));
            latest = Snapshot(now, 25, reset);
            store.Observe(latest);
            Assert(store.GetForecast(false, latest, now) == "No usage increase in recent history", "idle observations do not invent exhaustion");
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
            Assert(store.GetForecast(true, latest, now) == "\u22481.3 days left at current pace", "weekly wall-clock rate bridges overnight observations in a known cycle");
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
            store.Observe(Snapshot(now.AddMinutes(-15), 10, reset));
            latest = Snapshot(now, 20, reset);
            store.Observe(latest);
            samples = store.Samples;
            Assert(UsageHistoryStore.IsReset(samples[0], samples[1], false), "banked reset detected from a decrease");
            Assert(!UsageHistoryStore.IsContinuous(samples[0], samples[1], false), "chart does not connect across banked reset");
            Assert(store.GetForecast(false, latest, now) == "Learning usage pace...", "forecast cannot reuse consumption before reset");
            samples[1].FiveHour.UsedPercent = 95;
            samples[1].FiveHour.ResetAtUtc = reset.AddHours(5);
            Assert(UsageHistoryStore.IsReset(samples[0], samples[1], false), "changed reset deadline detects new cycle even when usage increased");
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
            Assert(store.GetForecast(true, latest, now) == "\u22484 days left at current pace", "80 percent remaining at 20 percent per day lasts four more days");
            Assert(store.GetForecast(true, latest, now.AddHours(1)) == "Refresh usage for an estimate", "stale observations cannot project depletion");
            latest.Weekly.ResetAfterSeconds = null;
            Assert(store.GetForecast(true, latest, now) == "\u22484 days left at current pace", "depletion uses observed consumption rather than time until reset");
            latest.Weekly.ResetAfterSeconds = 2 * 86400;
            Assert(store.GetForecast(true, latest, now) == "\u22484 days left at current pace (resets sooner)", "scheduled reset is supplementary to the estimate");
            latest.Weekly.ResetAfterSeconds = 0;
            Assert(store.GetForecast(true, latest, now) == "Awaiting reset update", "expired reset needs a fresh reading");
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
            Assert(store.GetForecast(true, current, now) == "\u22484 days left at current pace",
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
            Assert(store.GetForecast(true, snapshot, now) == "\u22484 days left at cycle average",
                "first reading of 20 percent in one day estimates four days without learning");
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
