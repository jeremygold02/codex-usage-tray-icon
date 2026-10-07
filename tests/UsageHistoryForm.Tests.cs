using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Reflection;
using System.Windows.Forms;

namespace CodexUsageTray.Tests
{
    internal static class UsageHistoryFormTests
    {
        public static void Run()
        {
            string path = Path.Combine(Path.GetTempPath(), "UnusedChartHistory-" + Guid.NewGuid().ToString("N"));
            using (UsageHistoryForm form = new UsageHistoryForm(new UsageHistoryStore(path), new AppSettings()))
            {
                Control chart = (Control)typeof(UsageHistoryForm).GetField("chart", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
                DateTime now = DateTime.UtcNow;
                List<UsageHistorySample> samples = new List<UsageHistorySample>
                {
                    Sample(now.AddDays(-2), 40),
                    Sample(now.AddDays(-8), 10),
                    new UsageHistorySample { TimestampUtc = now.AddDays(-10), FiveHour = new UsageHistoryWindow { UsedPercent = 20 } },
                    Sample(now.AddMinutes(1), 50)
                };
                Update(chart, samples, true, now);
                Assert(GetDate(chart, "startUtc") == now.AddDays(-8), "axis starts at the earliest weekly reading, even before seven days");
                Assert(GetDate(chart, "endUtc") == now, "axis ends at the current time");
                Assert(CountMiddleLinePixels(chart) > 10, "sparse readings are joined by visible line segments");

                samples[0].Weekly.UsedPercent = 0;
                Update(chart, samples, true, now);
                Assert(GetDate(chart, "startUtc") == samples[0].TimestampUtc && GetSamples(chart).Count == 1,
                    "weekly view starts at the first reading after the latest usage reset");
                Assert(CountMiddleLinePixels(chart) == 0, "a reset is not interpolated as gradual replenishment");

                Update(chart, samples, false, now);
                Assert(GetDate(chart, "startUtc") == now.AddDays(-10), "each usage window uses its own first reading");
                Update(chart, new List<UsageHistorySample> { Sample(now, 20) }, true, now);
                Assert(GetDate(chart, "startUtc") == now && GetDate(chart, "endUtc") > now, "single current reading has a nonzero range without earlier empty time");
                CountMiddleLinePixels(chart);
                Update(chart, new List<UsageHistorySample>(), true, now);
                Assert(GetDate(chart, "endUtc") > GetDate(chart, "startUtc"), "empty chart has a safe range");
                CountMiddleLinePixels(chart);
                TestInspectionAndProjection(chart, form, now);
                TestResetCycles(chart, form, now);
                TestAvailableLimits(form, now);
                TestPlateauInspection(chart, form, now);
                TestProjectionThroughReset(chart, form);
                TestAxisTicks(chart);
            }
        }

        private static void TestResetCycles(Control chart, UsageHistoryForm form, DateTime now)
        {
            List<UsageHistorySample> samples = new List<UsageHistorySample>
            {
                Sample(now.AddDays(-8), 10), Sample(now.AddDays(-7), 80),
                Sample(now.AddDays(-6), 0), Sample(now.AddDays(-5), 30)
            };
            Update(chart, samples, true, now);
            Assert(GetDate(chart, "startUtc") == samples[2].TimestampUtc && GetSamples(chart).Count == 2,
                "previous weekly cycles are excluded after sorting the readings");
            Raise(chart, "OnKeyDown", new KeyEventArgs(Keys.Home));
            Label detail = (Label)typeof(UsageHistoryForm).GetField("detailLabel", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
            Assert(detail.Text.Contains("100% remaining"), "keyboard inspection starts in the current cycle");

            samples.Add(Sample(now.AddHours(-2), 0));
            samples.Add(Sample(now.AddHours(-1), 20));
            Update(chart, samples, true, now);
            Assert(GetDate(chart, "startUtc") == samples[4].TimestampUtc && GetSamples(chart).Count == 2 &&
                detail.Text.Contains("Latest:") && detail.Text.Contains("80% remaining"),
                "another reset advances the chart and clears selection from the previous cycle");
            Assert(samples.Count == 6, "cropping the chart preserves retained source history");

            samples = new List<UsageHistorySample>
            {
                Sample(now.AddHours(-4), 20), Sample(now.AddHours(-3), 40),
                Sample(now.AddHours(-1), 45), Sample(now, 50)
            };
            DateTime oldReset = now.AddHours(-2);
            samples[0].Weekly.ResetAtUtc = oldReset;
            samples[1].Weekly.ResetAtUtc = oldReset;
            samples[2].Weekly.ResetAtUtc = oldReset.AddDays(7);
            samples[3].Weekly.ResetAtUtc = oldReset.AddDays(7);
            Update(chart, samples, true, now);
            Assert(GetDate(chart, "startUtc") == samples[2].TimestampUtc,
                "a new reset deadline starts a new cycle even if the percentage did not decrease");

            samples[0].Weekly.ResetAtUtc = now.AddDays(1);
            samples[1].Weekly.ResetAtUtc = now.AddDays(1).AddSeconds(30);
            samples[2].Weekly.ResetAtUtc = now.AddDays(1).AddSeconds(45);
            samples[3].Weekly.ResetAtUtc = now.AddDays(1).AddSeconds(60);
            Update(chart, samples, true, now);
            Assert(GetDate(chart, "startUtc") == samples[0].TimestampUtc,
                "small reset deadline variations do not crop valid readings");
        }

        private static void TestAvailableLimits(UsageHistoryForm form, DateTime now)
        {
            RadioButton weekly = (RadioButton)typeof(UsageHistoryForm).GetField("weeklyButton", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
            RadioButton fiveHour = (RadioButton)typeof(UsageHistoryForm).GetField("fiveHourButton", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
            UsageSnapshot snapshot = new UsageSnapshot
            {
                LastUpdated = now,
                Weekly = new LimitWindow { UsedPercent = 20, WindowMinutes = 10080 },
                FiveHour = new LimitWindow { UsedPercent = 10, WindowMinutes = 300 }
            };
            form.UpdateData(snapshot);
            Assert(!ContainsClearButton(form), "history window has no accidental clear-history action");
            form.Show();
            try
            {
                Assert(fiveHour.Visible, "five-hour selector is shown when the account reports a five-hour limit");
                fiveHour.Checked = true;
                snapshot.FiveHour = null;
                form.UpdateData(snapshot);
                Assert(!fiveHour.Visible && weekly.Checked,
                    "missing five-hour limit hides the selector and switches an open five-hour view to weekly");
                snapshot.FiveHour = new LimitWindow { UsedPercent = 10, WindowMinutes = 300 };
                form.UpdateData(snapshot);
                Assert(fiveHour.Visible && weekly.Checked, "five-hour selector returns when the limit becomes available");
            }
            finally { form.Hide(); }
        }

        private static bool ContainsClearButton(Control control)
        {
            if (control is Button && control.Text == "Clear history") return true;
            foreach (Control child in control.Controls)
                if (ContainsClearButton(child)) return true;
            return false;
        }

        private static void TestPlateauInspection(Control chart, UsageHistoryForm form, DateTime now)
        {
            List<UsageHistorySample> samples = new List<UsageHistorySample>
            {
                Sample(now.AddHours(-2), 15), Sample(now.AddHours(-1), 19),
                Sample(now.AddMinutes(-45), 19), Sample(now.AddMinutes(-30), 19), Sample(now, 19)
            };
            Update(chart, samples, true, now);
            Assert(GetSamples(chart).Count == 3,
                "repeated polls are coalesced while the first and latest plateau times remain available");
            Raise(chart, "OnKeyDown", new KeyEventArgs(Keys.End));
            Label detail = (Label)typeof(UsageHistoryForm).GetField("detailLabel", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
            Assert(detail.Text.Contains(now.AddHours(-1).ToLocalTime().ToString("G")),
                "inspection selects the percentage change rather than repeated unchanged polls");
            Raise(chart, "OnMouseLeave", EventArgs.Empty);
            Assert(detail.Text.Contains(now.ToLocalTime().ToString("G")), "latest-reading timestamp still stays current");

            UsageProjection projection = new UsageProjection
            {
                StartUtc = now, EndUtc = now.AddHours(2), StartRemainingPercent = 81, EndRemainingPercent = 60,
                Points = new List<UsageProjectionPoint>
                {
                    new UsageProjectionPoint { TimestampUtc = now, RemainingPercent = 81 },
                    new UsageProjectionPoint { TimestampUtc = now.AddHours(1), RemainingPercent = 80 },
                    new UsageProjectionPoint { TimestampUtc = now.AddHours(2), RemainingPercent = 60 }
                }
            };
            chart.GetType().GetMethod("UpdateProjection").Invoke(chart, new object[] { projection });
            CountMiddleLinePixels(chart);
            chart.GetType().GetMethod("SelectProjection", BindingFlags.Instance | BindingFlags.NonPublic)
                .Invoke(chart, new object[] { now.AddHours(1) });
            Assert(detail.Text.Contains("Projected:") && detail.Text.Contains("80% remaining"),
                "projected hover uses the curved path rather than a straight endpoint interpolation");
            Update(chart, samples, true, now);
            chart.GetType().GetMethod("UpdateProjection").Invoke(chart, new object[] { projection });
            Assert(detail.Text.Contains("Projected:") && detail.Text.Contains("80% remaining"),
                "refreshing chart data retains an active forecast inspection");
            chart.GetType().GetMethod("SelectProjection", BindingFlags.Instance | BindingFlags.NonPublic)
                .Invoke(chart, new object[] { now.AddMinutes(30) });
            Assert(detail.Text.Contains("81% remaining") && detail.Text.Contains("19% used") &&
                !detail.Text.Contains("80.5%") && !detail.Text.Contains("20% used"),
                "projected percentages round to whole numbers once and remain complementary");
            Assert(projection.Points[1].RemainingPercent == 80 && projection.EndUtc == now.AddHours(2),
                "display rounding does not change the forecast path or depletion time");
        }

        private static void TestInspectionAndProjection(Control chart, UsageHistoryForm form, DateTime now)
        {
            List<UsageHistorySample> samples = new List<UsageHistorySample>
            {
                Sample(now.AddHours(-4), 20), Sample(now.AddHours(-1), 40)
            };
            Update(chart, samples, true, now);
            CountMiddleLinePixels(chart);
            RectangleF plot = (RectangleF)chart.GetType().GetField("plot", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(chart);
            Label detail = (Label)typeof(UsageHistoryForm).GetField("detailLabel", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
            Assert(detail.Text.Contains("Latest:") && detail.Text.Contains("60% remaining"), "idle footer shows latest real reading");
            using (Bitmap before = new Bitmap(chart.Width, chart.Height))
            using (Bitmap after = new Bitmap(chart.Width, chart.Height))
            {
                chart.DrawToBitmap(before, new Rectangle(Point.Empty, before.Size));
                Raise(chart, "OnMouseMove", new MouseEventArgs(MouseButtons.Left, 0,
                    (int)(plot.Left + plot.Width * 0.30), (int)(plot.Top + 10), 0));
                Assert(detail.Text.Contains("80% remaining") && detail.Text.Contains("20% used"), "drag snaps to the nearest actual reading even across a large gap");
                chart.DrawToBitmap(after, new Rectangle(Point.Empty, after.Size));
                int changed = 0;
                for (int y = (int)plot.Top + 10; y < plot.Bottom - 10; y++)
                    if (before.GetPixel((int)plot.Left, y) != after.GetPixel((int)plot.Left, y)) changed++;
                Assert(changed > 40, "selected reading draws a full-height vertical guide");
            }
            Raise(chart, "OnKeyDown", new KeyEventArgs(Keys.End));
            Assert(detail.Text.Contains("60% remaining") && !detail.Text.Contains("Latest:"), "keyboard selection updates stats");
            Raise(chart, "OnMouseLeave", EventArgs.Empty);
            Assert(detail.Text.Contains("Latest:"), "leaving the chart restores latest-reading stats");

            UsageProjection projection = new UsageProjection
            {
                StartUtc = samples[1].TimestampUtc, EndUtc = now.AddHours(2),
                StartRemainingPercent = 60, EndRemainingPercent = 10, EndsAtReset = true
            };
            chart.GetType().GetMethod("UpdateProjection").Invoke(chart, new object[] { projection });
            Assert(GetDate(chart, "endUtc") == projection.EndUtc, "time axis includes the forecast endpoint");
            CountMiddleLinePixels(chart);
            plot = (RectangleF)chart.GetType().GetField("plot", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(chart);
            Raise(chart, "OnMouseMove", new MouseEventArgs(MouseButtons.None, 0, (int)plot.Right - 2, (int)plot.Top + 20, 0));
            Assert(detail.Text.Contains("Projected:") && detail.Text.Contains("remaining"), "forecast area shows an explicitly estimated reading at the hovered time");
            Raise(chart, "OnMouseLeave", EventArgs.Empty);
            Assert(detail.Text.Contains("Latest:") && detail.Text.Contains("60% remaining"),
                "leaving projected usage restores the latest recorded reading");
            projection.StartUtc = now;
            chart.GetType().GetMethod("UpdateProjection").Invoke(chart, new object[] { projection });
            Assert(GetDate(chart, "endUtc") == now, "projection unrelated to latest recorded point is hidden");
            Update(chart, new List<UsageHistorySample>(), true, now);
            Assert(detail.Text == "No recorded readings", "empty history clears selected stats");
        }

        private static void Raise(Control chart, string method, EventArgs args)
        {
            chart.GetType().GetMethod(method, BindingFlags.Instance | BindingFlags.NonPublic).Invoke(chart, new object[] { args });
        }

        private static void TestProjectionThroughReset(Control chart, UsageHistoryForm form)
        {
            DateTime reset = new DateTime(2026, 10, 9, 17, 0, 0, DateTimeKind.Local).ToUniversalTime();
            foreach (bool weekly in new[] { true, false })
            foreach (bool curved in new[] { false, true })
            {
                DateTime start = weekly ? reset.AddDays(-7).AddMinutes(10) : reset.AddHours(-5).AddMinutes(10);
                DateTime observed = weekly ? reset.AddDays(-3) : reset.AddHours(-3);
                DateTime depleted = weekly ? reset.AddDays(-1) : reset.AddHours(-1);
                List<UsageHistorySample> samples = new List<UsageHistorySample>
                {
                    Sample(start, 0), Sample(observed, 60)
                };
                foreach (UsageHistorySample sample in samples)
                {
                    UsageHistoryWindow window = sample.Weekly;
                    window.ResetAtUtc = reset;
                    if (!weekly) { sample.FiveHour = window; sample.Weekly = null; window.WindowMinutes = 300; }
                }
                Update(chart, samples, weekly, observed);
                Assert(GetDate(chart, "startUtc") == start && GetDate(chart, "endUtc") == reset,
                    "known reset sets the full chart range even before a forecast is available");
                UsageProjection projection = new UsageProjection
                {
                    StartUtc = observed, EndUtc = depleted, StartRemainingPercent = 40, EndRemainingPercent = 0,
                    Points = curved ? new List<UsageProjectionPoint>
                    {
                        new UsageProjectionPoint { TimestampUtc = observed, RemainingPercent = 40 },
                        new UsageProjectionPoint { TimestampUtc = depleted, RemainingPercent = 0 }
                    } : null
                };
                chart.GetType().GetMethod("UpdateProjection").Invoke(chart, new object[] { projection });
                Assert(GetDate(chart, "endUtc") == reset && projection.EndUtc == depleted,
                    "early depletion extends the display through reset without changing the forecast endpoint");
                CountMiddleLinePixels(chart);
                RectangleF plot = (RectangleF)chart.GetType().GetField("plot", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(chart);
                using (Bitmap image = new Bitmap(chart.Width, chart.Height))
                {
                    chart.DrawToBitmap(image, new Rectangle(Point.Empty, image.Size));
                    int orange = 0;
                    for (int x = (int)(plot.Right - plot.Width * 0.1); x < plot.Right - 2; x++)
                        for (int y = (int)plot.Bottom - 2; y <= (int)plot.Bottom; y++)
                        {
                            Color pixel = image.GetPixel(x, y);
                            if (pixel.R > pixel.G && pixel.G > pixel.B && pixel.B < 70) orange++;
                        }
                    Assert(orange > 20, "the projected zero plateau is visible through the reset for linear and curved paths");
                }
                int mouseX = (int)(plot.Right - plot.Width * 0.1);
                Raise(chart, "OnMouseMove", new MouseEventArgs(MouseButtons.None, 0, mouseX, (int)plot.Top + 20, 0));
                DateTime selected = (DateTime)chart.GetType().GetField("selectedProjectionUtc", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(chart);
                Label detail = (Label)typeof(UsageHistoryForm).GetField("detailLabel", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(form);
                Assert(selected > depleted && selected < reset && detail.Text.Contains("0% remaining") && detail.Text.Contains("100% used"),
                    "hover after depletion reports the hovered future time and zero remaining, rather than snapping back to depletion");
                Update(chart, samples, weekly, observed);
                chart.GetType().GetMethod("UpdateProjection").Invoke(chart, new object[] { projection });
                Assert(detail.Text.Contains(selected.ToLocalTime().ToString("G")),
                    "refresh preserves inspection on the projected zero plateau");
                chart.GetType().GetMethod("UpdateProjection").Invoke(chart, new object[] { null });
                Assert(GetDate(chart, "endUtc") == reset && detail.Text.Contains("Latest:"),
                    "hiding an unavailable forecast retains the reset range and clears its inspection");
                Update(chart, samples, weekly, reset.AddMinutes(1));
                Assert(GetDate(chart, "endUtc") == reset.AddMinutes(1), "expired deadlines do not move the time axis backwards");
            }
        }

        private static void TestAxisTicks(Control chart)
        {
            DateTime start = new DateTime(2026, 10, 2, 17, 13, 23, DateTimeKind.Local).ToUniversalTime();
            DateTime reset = new DateTime(2026, 10, 9, 17, 0, 0, DateTimeKind.Local).ToUniversalTime();
            UsageHistorySample sample = Sample(start, 10);
            sample.Weekly.ResetAtUtc = reset;
            foreach (int width in new[] { 420, 600, 1000 })
            {
                Update(chart, new List<UsageHistorySample> { sample }, true, start);
                chart.Size = new Size(width, 280);
                using (Bitmap image = new Bitmap(chart.Width, chart.Height))
                    chart.DrawToBitmap(image, new Rectangle(Point.Empty, image.Size));
                List<DateTime> ticks = (List<DateTime>)chart.GetType().GetMethod("GetAxisTicks", BindingFlags.Instance | BindingFlags.NonPublic)
                    .Invoke(chart, new object[] { 90f });
                Assert(ticks[0] == start && ticks[ticks.Count - 1] == reset,
                    "axis always labels the actual first reading and exact reset time");
                Assert(ticks.Count >= 3 && ticks.Count <= 10, "tick density adapts to chart width");
                for (int i = 1; i < ticks.Count - 1; i++)
                    Assert(ticks[i].ToLocalTime().TimeOfDay == TimeSpan.Zero && ticks[i] > ticks[i - 1],
                        "weekly interior ticks are ordered local calendar boundaries rather than arbitrary fractional dates");
            }
            start = reset.AddHours(-5).AddMinutes(13).AddSeconds(23);
            sample = new UsageHistorySample
            {
                TimestampUtc = start,
                FiveHour = new UsageHistoryWindow { UsedPercent = 10, WindowMinutes = 300, ResetAtUtc = reset }
            };
            Update(chart, new List<UsageHistorySample> { sample }, false, start);
            CountMiddleLinePixels(chart);
            List<DateTime> shortTicks = (List<DateTime>)chart.GetType().GetMethod("GetAxisTicks", BindingFlags.Instance | BindingFlags.NonPublic)
                .Invoke(chart, new object[] { 90f });
            Assert(shortTicks.Count > 2 && shortTicks[shortTicks.Count - 1] == reset, "five-hour view includes its reset label");
            for (int i = 1; i < shortTicks.Count - 1; i++)
                Assert(shortTicks[i].ToLocalTime().Minute == 0 && shortTicks[i].Second == 0,
                    "five-hour interior ticks use whole local hours");
        }

        private static UsageHistorySample Sample(DateTime timestamp, double used)
        {
            return new UsageHistorySample
            {
                TimestampUtc = timestamp,
                Weekly = new UsageHistoryWindow { UsedPercent = used, WindowMinutes = 10080 }
            };
        }

        private static void Update(Control chart, List<UsageHistorySample> samples, bool weekly, DateTime now)
        {
            chart.GetType().GetMethod("UpdateData").Invoke(chart, new object[] { samples, weekly, now });
        }

        private static DateTime GetDate(Control chart, string field)
        {
            return (DateTime)chart.GetType().GetField(field, BindingFlags.Instance | BindingFlags.NonPublic).GetValue(chart);
        }

        private static List<UsageHistorySample> GetSamples(Control chart)
        {
            return (List<UsageHistorySample>)chart.GetType().GetField("samples", BindingFlags.Instance | BindingFlags.NonPublic).GetValue(chart);
        }

        private static int CountMiddleLinePixels(Control chart)
        {
            chart.Size = new Size(600, 280);
            chart.GetType().GetMethod("ApplyTheme").Invoke(chart, new object[] { false });
            using (Bitmap image = new Bitmap(chart.Width, chart.Height))
            {
                chart.DrawToBitmap(image, new Rectangle(Point.Empty, image.Size));
                int count = 0;
                for (int x = 200; x < 300; x++)
                    for (int y = 30; y < 200; y++)
                        if (image.GetPixel(x, y).ToArgb() == Color.FromArgb(0, 104, 180).ToArgb()) count++;
                return count;
            }
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("History chart: " + message);
        }
    }
}
