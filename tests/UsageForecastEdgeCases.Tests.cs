using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Reflection;
using System.Windows.Forms;

namespace CodexUsageTray.Tests
{
    internal static class UsageForecastEdgeCasesTests
    {
        public static void Run()
        {
            string directory = Path.Combine(Path.GetTempPath(), "CodexForecastEdges-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(directory);
            try
            {
                TestCycleAverageJitter(Path.Combine(directory, "jitter.json"));
                TestResetIsolation(Path.Combine(directory, "reset.json"));
                TestHistoryCutoff(Path.Combine(directory, "cutoff.json"));
                TestMinimumSpan(Path.Combine(directory, "span.json"));
                TestFiveHourIntervalSplitting(Path.Combine(directory, "splitting.json"));
                TestSubsecondIntervals(Path.Combine(directory, "subsecond.json"));
                TestChartCycleAverageJitter(Path.Combine(directory, "chart.json"));
            }
            finally
            {
                foreach (string file in Directory.GetFiles(directory)) File.Delete(file);
                Directory.Delete(directory);
            }
        }

        private static void TestCycleAverageJitter(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            foreach (bool weekly in new[] { true, false })
            {
                DateTime reset = weekly ? now.AddDays(6) : now.AddHours(4);
                double used = weekly ? 20 : 25;
                foreach (double decline in new[] { 0.0005, 0.001 })
                {
                    store.Clear();
                    Observe(store, Snapshot(now.AddMinutes(-30), used + decline, reset));
                    UsageSnapshot latest = Snapshot(now, used, reset);
                    Observe(store, latest);
                    List<UsageHistorySample> samples = store.Samples;
                    Assert(!UsageHistoryStore.IsReset(samples[0], samples[1], weekly),
                        "a decline within the reset tolerance is not a reset");
                    string expected = weekly ? "\u22484 days left at cycle average" : "\u22483 hours left at cycle average";
                    Assert(store.GetForecast(weekly, latest, now) == expected,
                        "sub-tolerance declines must preserve the cycle-average forecast for " + (weekly ? "weekly" : "five-hour"));
                    UsageProjection projection = store.GetProjection(weekly, latest, now);
                    double hours = weekly ? 96 : 3;
                    Assert(projection != null && !projection.EndsAtReset &&
                        Math.Abs((projection.EndUtc - now).TotalHours - hours) < 0.001,
                        "cycle-average projection survives tolerated measurement jitter");
                }

                store.Clear();
                Observe(store, Snapshot(now.AddMinutes(-30), used + 0.002, reset));
                UsageSnapshot afterAdjustment = Snapshot(now, used, reset);
                Observe(store, afterAdjustment);
                Assert(store.GetForecast(weekly, afterAdjustment, now) == "Learning usage pace..." &&
                    store.GetProjection(weekly, afterAdjustment, now) == null,
                    "a decrease beyond the tolerance still invalidates the inferred cycle start");
            }
        }

        private static void TestResetIsolation(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            foreach (bool weekly in new[] { true, false })
            {
                DateTime reset = weekly ? now.AddDays(3) : now.AddHours(4);
                double stepHours = weekly ? 6 : 0.25;
                foreach (bool changedDeadline in new[] { false, true })
                {
                    store.Clear();
                    // A banked reset decreases usage; a scheduled reset changes its deadline.
                    Observe(store, Snapshot(now.AddHours(-3 * stepHours), changedDeadline ? 0 : 95,
                        changedDeadline ? now.AddHours(-2.5 * stepHours) : reset));
                    Observe(store, Snapshot(now.AddHours(-2 * stepHours), 10, reset));
                    Observe(store, Snapshot(now.AddHours(-stepHours), weekly ? 16 : 20, reset));
                    UsageSnapshot latest = Snapshot(now, weekly ? 40 : 30, reset);
                    Observe(store, latest);
                    List<UsageHistorySample> samples = store.Samples;
                    Assert(samples.Count == 3 && samples[0].TimestampUtc == now.AddHours(-2 * stepHours),
                        "the old cycle is pruned at the reset boundary");
                    UsageProjection projection = store.GetProjection(weekly, latest, now);
                    double expectedHours = weekly ? 20 : 1.75;
                    Assert(projection != null && !projection.EndsAtReset &&
                        Math.Abs((projection.EndUtc - now).TotalHours - expectedHours) < 0.001,
                        "weighted pace uses only observations after a banked or scheduled reset");
                    Assert(store.GetForecast(weekly, latest, now) ==
                        (weekly ? "\u224820 hours left at current pace" : "\u22482 hours left at current pace"),
                        "forecast text and projection agree after the reset");
                }
            }
        }

        private static void TestHistoryCutoff(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            foreach (bool weekly in new[] { true, false })
            {
                store.Clear();
                DateTime reset = weekly ? now.AddDays(3) : now.AddHours(4);
                if (!weekly) reset = now.AddHours(3);
                double horizon = weekly ? 48 : 1;
                Observe(store, Snapshot(now.AddHours(-horizon - 0.25), 0, reset));
                Observe(store, Snapshot(now.AddHours(-horizon), 50, reset));
                Observe(store, Snapshot(now.AddHours(-horizon / 2), 50, reset));
                UsageSnapshot latest = Snapshot(now, 50, reset);
                Observe(store, latest);
                Assert(store.GetForecast(weekly, latest, now) == "No usage increase in recent history",
                    "a burst ending at the cutoff must not affect subsequent idle observations");
                UsageProjection projection = store.GetProjection(weekly, latest, now);
                Assert(projection != null && projection.EndsAtReset && projection.EndRemainingPercent == 50,
                    "old consumption outside the recent horizon does not slope the projection");
            }
        }

        private static void TestMinimumSpan(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            UsageHistoryStore store = new UsageHistoryStore(path);
            foreach (bool weekly in new[] { true, false })
            {
                DateTime reset = weekly ? now.AddDays(3) : now.AddHours(4);
                double spanHours = weekly ? 6 : 0.25;
                foreach (bool sufficient in new[] { false, true })
                {
                    store.Clear();
                    double span = sufficient ? spanHours : spanHours - 1.0 / 3600;
                    Observe(store, Snapshot(now.AddHours(-span), 0, reset));
                    Observe(store, Snapshot(now.AddHours(-span / 2), 6, reset));
                    UsageSnapshot latest = Snapshot(now, 12, reset);
                    Observe(store, latest);
                    string forecast = store.GetForecast(weekly, latest, now);
                    if (sufficient)
                    {
                        UsageProjection projection = store.GetProjection(weekly, latest, now);
                        Assert(forecast.EndsWith("left at current pace") && projection != null &&
                            Math.Abs((projection.EndUtc - now).TotalHours - 88 * spanHours / 12) < 0.001,
                            "the exact minimum span enables recent weighted pace");
                    }
                    else
                        Assert(forecast == "Will last until reset", "a span just below the minimum uses the cycle average");
                }
            }
        }

        private static void TestFiveHourIntervalSplitting(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime reset = now.AddHours(4);
            UsageHistoryStore store = new UsageHistoryStore(path);
            double[][] times = { new[] { -60.0, -30, 0 }, new[] { -60.0, -50, -40, -30, -29, -15, -1, 0 } };
            double[][] usage = { new[] { 0.0, 6, 30 }, new[] { 0.0, 2, 4, 6, 6.8, 18, 29.2, 30 } };
            for (int scenario = 0; scenario < times.Length; scenario++)
            {
                store.Clear();
                for (int i = 0; i < times[scenario].Length; i++)
                    Observe(store, Snapshot(now.AddMinutes(times[scenario][i]), usage[scenario][i], reset));
                UsageSnapshot latest = Snapshot(now, 30, reset);
                UsageProjection projection = store.GetProjection(false, latest, now);
                Assert(projection != null && Math.Abs((projection.EndUtc - now).TotalHours - 70.0 / 40.8) < 0.001,
                    "irregular five-hour polling preserves time-weighted acceleration");
            }
        }

        private static void TestSubsecondIntervals(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime reset = now.AddHours(4);
            UsageHistoryStore store = new UsageHistoryStore(path);
            Observe(store, Snapshot(now.AddMinutes(-30), 20, reset));
            Observe(store, Snapshot(now.AddMinutes(-15), 35, reset));
            Observe(store, Snapshot(now.AddMilliseconds(-1), 50 - 1.0 / 60000, reset));
            UsageSnapshot latest = Snapshot(now, 50, reset);
            Observe(store, latest);
            UsageProjection projection = store.GetProjection(false, latest, now);
            Assert(projection != null && Math.Abs((projection.EndUtc - now).TotalHours - 50.0 / 60) < 0.001,
                "a millisecond interval keeps a finite steady forecast rate");
            Assert(!store.Observe(latest), "duplicate timestamps never reach the interval division");
        }

        private static void TestChartCycleAverageJitter(string path)
        {
            DateTime now = DateTime.UtcNow.AddSeconds(-1);
            DateTime reset = now.AddDays(6);
            UsageHistoryStore store = new UsageHistoryStore(path);
            Observe(store, Snapshot(now.AddHours(-1), 20.0005, reset));
            UsageSnapshot latest = Snapshot(now, 20, reset);
            Observe(store, latest);
            using (UsageHistoryForm form = new UsageHistoryForm(store, new AppSettings()))
            {
                form.UpdateData(latest);
                Label forecast = (Label)typeof(UsageHistoryForm).GetField("forecastLabel", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
                Assert(forecast.Text == "\u22484 days left at cycle average", "chart footer retains its estimate after harmless jitter");
                Control chart = (Control)typeof(UsageHistoryForm).GetField("chart", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
                UsageProjection projection = (UsageProjection)chart.GetType().GetField("projection", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(chart);
                Assert(projection != null && projection.StartUtc == now && projection.EndUtc == now.AddDays(4),
                    "chart keeps the cycle-average projection attached to the latest real reading");
                chart.Size = new Size(600, 280);
                chart.GetType().GetMethod("ApplyTheme").Invoke(chart, new object[] { false });
                using (Bitmap image = new Bitmap(chart.Width, chart.Height))
                {
                    chart.DrawToBitmap(image, new Rectangle(Point.Empty, image.Size));
                    int amberPixels = 0;
                    for (int x = image.Width / 2; x < image.Width - 25; x++)
                        for (int y = 50; y < image.Height - 40; y++)
                            if (image.GetPixel(x, y).ToArgb() == Color.FromArgb(160, 89, 0).ToArgb()) amberPixels++;
                    Assert(amberPixels > 20, "the projection is visibly rendered after tolerated jitter");
                }
            }
        }

        private static UsageSnapshot Snapshot(DateTime timestamp, double used, DateTime reset)
        {
            int seconds = (int)(reset - timestamp).TotalSeconds;
            return new UsageSnapshot
            {
                LastUpdated = timestamp,
                LastAttempted = timestamp,
                Weekly = new LimitWindow { UsedPercent = used, WindowMinutes = 10080, ResetAfterSeconds = seconds },
                FiveHour = new LimitWindow { UsedPercent = used, WindowMinutes = 300, ResetAfterSeconds = seconds }
            };
        }

        private static void Observe(UsageHistoryStore store, UsageSnapshot snapshot)
        {
            Assert(store.Observe(snapshot), "edge-case observation is accepted");
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("Forecast edge cases: " + message);
        }
    }
}
