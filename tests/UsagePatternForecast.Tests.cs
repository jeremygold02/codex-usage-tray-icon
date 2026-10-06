using System;
using System.Collections.Generic;
using System.IO;

namespace CodexUsageTray.Tests
{
    internal static class UsagePatternForecastTests
    {
        public static void Run()
        {
            DateTime latestTime = DateTime.UtcNow.Date.AddDays(-1).AddHours(16);
            List<UsageHistorySample> samples = Pattern(latestTime, 9, false);
            double baseline = RecentRate(samples);
            UsagePatternForecast model = UsagePatternForecast.TryCreate(samples, baseline);
            Assert(model != null && model.ValidationCount >= 8 && model.PatternError < model.PaceError * 0.9,
                "a recurring daily pattern is selected only after beating the pace forecast on held-out readings");
            UsageProjection projection = model.Project(latestTime, latestTime.AddDays(2), 20);
            Assert(projection.Points.Count > 2 && projection.Points[1].RemainingPercent == 20 &&
                projection.EndsAtReset && Math.Abs(projection.EndRemainingPercent - 4) < 0.001,
                "quiet hours are flat and busy hours consume observed allowance until reset");
            UsageProjection depleted = model.Project(latestTime, latestTime.AddDays(3), 10.5);
            Assert(!depleted.EndsAtReset && depleted.EndRemainingPercent == 0 &&
                depleted.EndUtc == latestTime.Date.AddDays(2).AddHours(10.5),
                "daily pattern stops exactly at depletion inside a busy hour");
            double previous = double.MaxValue;
            foreach (UsageProjectionPoint point in depleted.Points)
            {
                Assert(point.RemainingPercent >= 0 && point.RemainingPercent <= previous,
                    "projection never invents replenishment or negative allowance");
                previous = point.RemainingPercent;
            }

            List<UsageHistorySample> steady = Pattern(latestTime, 9, true);
            Assert(UsagePatternForecast.TryCreate(steady, RecentRate(steady)) == null,
                "steady consumption keeps the simpler forecast when seasonal prediction has no advantage");
            List<UsageHistorySample> sparse = new List<UsageHistorySample>();
            for (int i = 0; i < steady.Count; i += 6) sparse.Add(steady[i]);
            Assert(UsagePatternForecast.TryCreate(sparse, RecentRate(sparse)) == null,
                "long positive gaps cannot be used to fabricate hourly usage patterns");
            List<UsageHistorySample> shortHistory = samples.GetRange(samples.Count - 49, 49);
            UsagePatternForecast shortModel = UsagePatternForecast.TryCreate(shortHistory, RecentRate(shortHistory));
            Assert(shortModel == null || !shortModel.UsesDailyPattern,
                "two days of observations do not establish a recurring pattern");

            TestStoreAgreement(Pattern(latestTime, 6, false), latestTime);
            TestInactiveRoutine();
        }

        private static void TestStoreAgreement(List<UsageHistorySample> samples, DateTime latestTime)
        {
            string path = Path.Combine(Path.GetTempPath(), "UsagePatternTest-" + Guid.NewGuid().ToString("N") + ".json");
            try
            {
                UsageHistoryStore store = new UsageHistoryStore(path);
                List<UsageSnapshot> readings = new List<UsageSnapshot>();
                foreach (UsageHistorySample sample in samples) readings.Add(Snapshot(sample));
                store.Import(readings);
                UsageSnapshot latest = readings[readings.Count - 1];
                UsageProjection projection = store.GetProjection(true, latest, latestTime);
                Assert(projection != null && projection.UsesDailyPattern && projection.Points.Count > 2,
                    "store returns a validated path instead of an artificial curved trend");
                Assert(store.GetForecast(true, latest, latestTime) == "Will last until reset",
                    "forecast text and seasonal path agree about lasting through reset");
                Assert(store.GetProjection(true, latest, latestTime.AddMinutes(31)) == null,
                    "stale snapshots still suppress the seasonal forecast");
                store.Clear();
                projection = store.GetProjection(true, latest, latestTime);
                Assert(projection != null && !projection.UsesDailyPattern && projection.Points == null,
                    "clearing history discards learned patterns and restores the cycle-average fallback");
            }
            finally
            {
                if (File.Exists(path)) File.Delete(path);
                if (File.Exists(path + ".imported")) File.Delete(path + ".imported");
            }
        }

        private static List<UsageHistorySample> Pattern(DateTime latest, int days, bool steady)
        {
            List<UsageHistorySample> samples = new List<UsageHistorySample>();
            DateTime first = latest.AddDays(-days);
            DateTime reset = first.AddDays(7);
            double used = 0;
            for (DateTime time = first; time <= latest; time = time.AddHours(1))
            {
                if (time >= reset)
                {
                    used = 0;
                    reset = reset.AddDays(7);
                }
                samples.Add(new UsageHistorySample
                {
                    TimestampUtc = time,
                    Weekly = new UsageHistoryWindow { UsedPercent = used, WindowMinutes = 10080, ResetAtUtc = reset }
                });
                used += steady ? 1.0 / 3 : (time.Hour >= 8 && time.Hour < 16 ? 1 : 0);
            }
            return samples;
        }

        private static void TestInactiveRoutine()
        {
            DateTime friday = DateTime.Now.Date.AddDays(-1);
            while (friday.DayOfWeek != DayOfWeek.Friday) friday = friday.AddDays(-1);
            DateTime latest = friday.AddHours(12).ToUniversalTime();
            List<UsageHistorySample> samples = Routine(latest);
            UsagePatternForecast model = UsagePatternForecast.TryCreate(samples, RecentRate(samples));
            Assert(model != null && model.UsesInactiveHours && model.Rates.Length == 48,
                "weekday and weekend inactivity can beat constant recent pace on held-out readings");
            DateTime saturday = friday.AddDays(1).ToUniversalTime();
            DateTime monday = friday.AddDays(3).AddHours(8).ToUniversalTime();
            Assert(UsagePatternForecast.Consumption(model.Rates, saturday, monday) == 0,
                "repeated quiet nights and weekends do not consume projected allowance");
            Assert(UsagePatternForecast.Consumption(model.Rates, monday, monday.AddHours(1)) > 0.45,
                "work resumes at the recent increased pace rather than the older working rate");
            UsageProjection projection = model.Project(latest, monday.AddHours(4), 80);
            Assert(projection.UsesInactiveHours && projection.Points.Count > 24 && projection.EndsAtReset,
                "inactivity-aware projection includes hourly points for hover inspection");
            DateTime night = friday.AddDays(-2).AddHours(2).ToUniversalTime();
            List<UsageHistorySample> unusualActivity = Routine(night.AddHours(1));
            unusualActivity.RemoveAt(unusualActivity.Count - 1);
            unusualActivity[unusualActivity.Count - 1].Weekly.UsedPercent += 0.9;
            UsagePatternForecast activeAtNight = UsagePatternForecast.TryCreate(unusualActivity, RecentRate(unusualActivity));
            Assert(activeAtNight != null && activeAtNight.UsesInactiveHours && activeAtNight.StartHourRate > 0,
                "actual recent activity overrides a learned quiet hour at the forecast start");
            projection = activeAtNight.Project(night, night.AddHours(2), 80);
            Assert(projection.Points[1].RemainingPercent < 80,
                "working during usual sleep hours consumes allowance immediately");

            // A new weekend reset must retain the previous weeks' learned routine.
            samples = Routine(saturday.AddHours(12));
            UsageHistorySample firstAfterReset = samples[samples.Count - 1];
            firstAfterReset.Weekly.UsedPercent = 0;
            firstAfterReset.Weekly.ResetAtUtc = firstAfterReset.TimestampUtc.AddDays(7);
            string path = Path.Combine(Path.GetTempPath(), "RoutineTest-" + Guid.NewGuid().ToString("N") + ".json");
            try
            {
                UsageHistoryStore store = new UsageHistoryStore(path);
                List<UsageSnapshot> readings = new List<UsageSnapshot>();
                foreach (UsageHistorySample sample in samples) readings.Add(Snapshot(sample));
                store.Import(readings);
                UsageSnapshot snapshot = Snapshot(firstAfterReset);
                projection = store.GetProjection(true, snapshot, firstAfterReset.TimestampUtc);
                Assert(projection != null && projection.UsesInactiveHours && projection.Points.Count > 2,
                    "one reading after reset can reuse prior-cycle patterns without relearning from zero");
                Assert(store.Samples[0].TimestampUtc < firstAfterReset.TimestampUtc.AddDays(-7),
                    "learning retains prior weeks across resets");
                store = new UsageHistoryStore(path);
                projection = store.GetProjection(true, snapshot, firstAfterReset.TimestampUtc);
                Assert(projection != null && projection.UsesInactiveHours,
                    "learned inactivity survives restart through retained usage readings");
            }
            finally
            {
                if (File.Exists(path)) File.Delete(path);
                if (File.Exists(path + ".imported")) File.Delete(path + ".imported");
            }
        }

        private static List<UsageHistorySample> Routine(DateTime latest)
        {
            List<UsageHistorySample> samples = new List<UsageHistorySample>();
            DateTime first = latest.AddDays(-21);
            DateTime reset = first.AddDays(7);
            double used = 0;
            for (DateTime time = first; time <= latest; time = time.AddHours(1))
            {
                if (time >= reset) { used = 0; reset = reset.AddDays(7); }
                samples.Add(new UsageHistorySample
                {
                    TimestampUtc = time,
                    Weekly = new UsageHistoryWindow { UsedPercent = used, WindowMinutes = 10080, ResetAtUtc = reset }
                });
                DateTime local = time.ToLocalTime();
                bool weekend = local.DayOfWeek == DayOfWeek.Saturday || local.DayOfWeek == DayOfWeek.Sunday;
                if (!weekend && local.Hour >= 8 && local.Hour < 20)
                    used += time >= latest.AddDays(-2) ? 0.6 : 0.3;
            }
            return samples;
        }

        private static double RecentRate(List<UsageHistorySample> samples)
        {
            double rate;
            UsageHistorySample latest = samples[samples.Count - 1];
            if (!UsageHistoryStore.TryGetRecentRate(true, Snapshot(latest), samples, out rate))
                UsageHistoryStore.TryGetCycleAverageRate(true, Snapshot(latest), samples, out rate);
            return rate;
        }

        private static UsageSnapshot Snapshot(UsageHistorySample sample)
        {
            return new UsageSnapshot
            {
                LastUpdated = sample.TimestampUtc,
                Weekly = new LimitWindow
                {
                    UsedPercent = sample.Weekly.UsedPercent, WindowMinutes = sample.Weekly.WindowMinutes,
                    ResetAfterSeconds = (int)(sample.Weekly.ResetAtUtc.Value - sample.TimestampUtc).TotalSeconds
                }
            };
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("Usage patterns: " + message);
        }
    }
}
