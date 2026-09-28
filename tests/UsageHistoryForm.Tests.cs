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
                Assert(CountMiddleLinePixels(chart) == 0, "a reset is not interpolated as gradual replenishment");

                Update(chart, samples, false, now);
                Assert(GetDate(chart, "startUtc") == now.AddDays(-10), "each usage window uses its own first reading");
                Update(chart, new List<UsageHistorySample> { Sample(now, 20) }, true, now);
                Assert(GetDate(chart, "startUtc") == now && GetDate(chart, "endUtc") > now, "single current reading has a nonzero range without earlier empty time");
                CountMiddleLinePixels(chart);
                Update(chart, new List<UsageHistorySample>(), true, now);
                Assert(GetDate(chart, "endUtc") > GetDate(chart, "startUtc"), "empty chart has a safe range");
                CountMiddleLinePixels(chart);
            }
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
