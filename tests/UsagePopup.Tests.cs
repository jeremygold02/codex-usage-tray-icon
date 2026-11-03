using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Reflection;
using System.Windows.Forms;

namespace CodexUsageTray.Tests
{
    internal static class UsagePopupTests
    {
        public static void Run()
        {
            TestObservedDailyUsage();
            TestResetEntrySpacing();
            UsageHistoryStore history = new UsageHistoryStore(Path.Combine(Path.GetTempPath(),
                "PopupTests-" + Guid.NewGuid().ToString("N") + ".json"));
            using (UsagePopup popup = new UsagePopup(new AppSettings(), history))
            {
                UsageSnapshot snapshot = new UsageSnapshot
                {
                    LastUpdated = DateTime.Now,
                    Weekly = new LimitWindow { UsedPercent = 25, WindowMinutes = 10080, ResetAfterSeconds = 86400 }
                };
                popup.UpdateSnapshot(snapshot);
                popup.UpdateClaudeSnapshot(snapshot.Clone(), history);
                Font bodyFont = Field<Font>(popup, "detailFont");
                Font resetFont = Field<Font>(popup, "resetDetailFont");
                Font barFont = Field<Font>(popup, "percentFont");
                Assert(resetFont.SizeInPoints < bodyFont.SizeInPoints && barFont.SizeInPoints < bodyFont.SizeInPoints &&
                    resetFont.SizeInPoints >= 9 && barFont.SizeInPoints >= 9,
                    "reset details and progress labels remain readable and proportionate to the body text");
                Button refresh = Field<Button>(popup, "refreshButton");
                Button provider = Field<Button>(popup, "providerButton");
                Timer animation = Field<Timer>(popup, "refreshAnimationTimer");

                popup.SetClaudeRefreshing(true);
                Assert(refresh.Enabled && !animation.Enabled, "Claude refresh does not animate the Codex view");
                Click(provider);
                Assert(popup.IsShowingClaude && !refresh.Enabled && animation.Enabled &&
                    refresh.AccessibleName == "Refreshing usage", "Claude view starts its spinner and disables duplicate clicks");
                int before = Field<int>(popup, "refreshAnimationAngle");
                typeof(UsagePopup).GetMethod("RefreshAnimationTimer_Tick", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, new object[] { null, EventArgs.Empty });
                Assert(Field<int>(popup, "refreshAnimationAngle") != before, "refresh spinner advances");

                popup.SetRefreshing(true);
                popup.SetClaudeRefreshing(false);
                Assert(refresh.Enabled && !animation.Enabled && refresh.AccessibleName == "Refresh Claude account usage",
                    "Claude completion stops its spinner independently of Codex refresh");
                snapshot.StatusMessage = "Claude usage updated";
                popup.UpdateClaudeSnapshot(snapshot, history);
                Assert((string)typeof(UsagePopup).GetMethod("BuildStatusLine", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, null) == "Claude usage updated", "successful manual refresh has visible confirmation");
                Click(provider);
                Assert(!refresh.Enabled && animation.Enabled, "switching back preserves the active Codex refresh");
                popup.SetRefreshing(false);
                Assert(refresh.Enabled && !animation.Enabled, "all animations stop when their refresh completes");
                foreach (int used in new[] { 6, 65 })
                {
                    snapshot.Weekly.UsedPercent = used;
                    popup.UpdateSnapshot(snapshot);
                    popup.UpdateClaudeSnapshot(snapshot.Clone(), history);
                    using (Bitmap codexBar = RenderBar(popup))
                    {
                        Click(provider);
                        using (Bitmap claudeBar = RenderBar(popup))
                        {
                            for (int y = 63; y < 81; y++)
                                for (int x = 0; x < codexBar.Width; x++)
                                    Assert(codexBar.GetPixel(x, y) == claudeBar.GetPixel(x, y),
                                        "Claude and Codex render identical remaining percentages and bars");
                            int width = popup.ClientSize.Width - 32;
                            int boundary = 10 + (int)Math.Round(width * ((100 - used) / 100.0));
                            Color track = claudeBar.GetPixel(10 + width - 8, 67);
                            Assert(claudeBar.GetPixel(boundary - 3, 67) != track &&
                                claudeBar.GetPixel(boundary + 3, 67) == track,
                                "the bar fills 94 percent for 6 used and 35 percent for 65 used");
                        }
                    }
                    Click(provider);
                }

                snapshot.Weekly.UsedPercent = 100;
                popup.UpdateSnapshot(snapshot);
                Color emptyColor;
                using (Bitmap empty = RenderBar(popup))
                {
                    emptyColor = empty.GetPixel(50, 67);
                    Assert(empty.GetPixel(50, 67) == empty.GetPixel(200, 67),
                        "zero remaining leaves the progress track empty");
                }
                snapshot.Weekly.UsedPercent = 0;
                popup.UpdateSnapshot(snapshot);
                using (Bitmap full = RenderBar(popup))
                    Assert(full.GetPixel(50, 67) != emptyColor && full.GetPixel(50, 67) == full.GetPixel(200, 67),
                        "one hundred percent remaining fills the progress track");

                int oneRowHeight = popup.ClientSize.Height;
                snapshot.FiveHour = new LimitWindow { UsedPercent = 50, WindowMinutes = 300, ResetAfterSeconds = 3600 };
                popup.UpdateSnapshot(snapshot);
                Assert(popup.ClientSize.Height > oneRowHeight,
                    "adding a second usage card increases popup height");
                int twoRowsHeight = popup.ClientSize.Height;
                snapshot.StatusMessage = "A deliberately long status message that must stay on its own line";
                snapshot.AvailableResetCount = 2;
                snapshot.AvailableResets.Add(new RateLimitResetCredit { Title = "A long reset title", ExpiresAtUtc = DateTime.UtcNow.AddDays(2) });
                popup.UpdateSnapshot(snapshot);
                Assert(popup.ClientSize.Height > twoRowsHeight &&
                    (int)typeof(UsagePopup).GetMethod("GetResetDetailLineCount", BindingFlags.NonPublic | BindingFlags.Instance)
                        .Invoke(popup, null) == 3,
                    "status and itemized or unitemized resets reserve separate footer lines");
                snapshot.StatusMessage = "This long status describes the connection issue in enough detail that it must wrap onto another line instead of becoming unreadable or overlapping the reset details.";
                snapshot.AvailableResets[0].Title = "A particularly long reset credit title that must remain visible when the expiry date is also shown";
                popup.UpdateSnapshot(snapshot);
                Assert((int)typeof(UsagePopup).GetMethod("GetStatusLineCount", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, null) > 1 &&
                    (int)typeof(UsagePopup).GetMethod("GetResetDetailLineCount", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, null) > 3,
                    "long status and reset details reserve wrapped lines");
                Assert(popup.AccessibleDescription.Contains(snapshot.StatusMessage) &&
                    popup.AccessibleDescription.Contains(snapshot.AvailableResets[0].Title) &&
                    popup.AccessibleDescription.Contains("Weekly: 100% remaining") &&
                    popup.AccessibleDescription.Contains("5-hour: 50% remaining"),
                    "full status, reset titles and remaining percentages remain available to assistive technology");
                ToolTip popupToolTips = Field<ToolTip>(popup, "actionToolTip");
                Assert(string.IsNullOrEmpty(popupToolTips.GetToolTip(popup)) &&
                    !string.IsNullOrEmpty(popupToolTips.GetToolTip(refresh)),
                    "no popup-wide tooltip appears while action-specific tooltips remain available");
                string extendedText = string.Concat(new string[] {
                    "A long detail that needs to remain visible even when it takes several lines. ",
                    "A long detail that needs to remain visible even when it takes several lines. ",
                    "A long detail that needs to remain visible even when it takes several lines. ",
                    "A long detail that needs to remain visible even when it takes several lines. " });
                snapshot.StatusMessage = extendedText;
                snapshot.AvailableResets[0].Title = extendedText;
                popup.UpdateSnapshot(snapshot);
                Assert((int)typeof(UsagePopup).GetMethod("GetStatusLineCount", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, null) > 3 &&
                    (int)typeof(UsagePopup).GetMethod("GetResetDetailLineCount", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, null) > 5,
                    "long status and reset titles reserve more than three lines instead of truncating");
                Assert(popup.GetUsageRowHeightForText("Next reset: " + extendedText, "Estimated: " + extendedText) > 104,
                    "long reset and forecast values expand the usage card");
                Assert(UsagePopup.GetTextScale(9) == 1 && Math.Abs(UsagePopup.GetTextScale(15) - 1.5) < 0.001,
                    "Windows message fonts above the default enlarge the popup text and layout");

                MethodInfo scaleMetric = typeof(UsagePopup).GetMethod("ScaleMetric", BindingFlags.NonPublic | BindingFlags.Instance);
                MethodInfo wrappedLines = typeof(UsagePopup).GetMethod("GetWrappedLineCount", BindingFlags.NonPublic | BindingFlags.Instance);
                int wideWidth = (int)scaleMetric.Invoke(popup, new object[] { 492 });
                int scrolledWidth = (int)scaleMetric.Invoke(popup, new object[] { 468 });
                string nearBoundary = "";
                int narrowLines = 0;
                for (int word = 0; word < 200; word++)
                {
                    nearBoundary += "sample ";
                    int wideLines = (int)wrappedLines.Invoke(popup, new object[] { nearBoundary, wideWidth });
                    narrowLines = (int)wrappedLines.Invoke(popup, new object[] { nearBoundary, scrolledWidth });
                    if (narrowLines > wideLines) break;
                }
                Assert(narrowLines > (int)wrappedLines.Invoke(popup, new object[] { nearBoundary, wideWidth }),
                    "test data crosses a wrapping boundary when the scrollbar appears");
                snapshot.StatusMessage = nearBoundary;
                popup.UpdateSnapshot(snapshot);
                Assert((int)typeof(UsagePopup).GetMethod("GetStatusLineCount", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, null) >= narrowLines,
                    "status height reserves the narrower scrollbar viewport before it appears");
                int titleWidth = (int)scaleMetric.Invoke(popup, new object[] { 460 });
                MethodInfo resetWrappedLines = typeof(UsagePopup).GetMethod("GetResetWrappedLineCount", BindingFlags.NonPublic | BindingFlags.Instance);
                int titleLines = (int)resetWrappedLines.Invoke(popup, new object[] { nearBoundary, titleWidth });
                Assert((int)typeof(UsagePopup).GetMethod("GetResetEntryLineCount", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, new object[] { nearBoundary, "Expires Nov 30, 10:30 PM" }) >= titleLines + 1,
                    "reset title and expiry reserve enough lines for the narrower scrollbar viewport");
                snapshot.StatusMessage = extendedText;

                for (int index = 2; index <= 20; index++)
                    snapshot.AvailableResets.Add(new RateLimitResetCredit { Title = "Reset credit " + index });
                snapshot.AvailableResetCount = 20;
                popup.UpdateSnapshot(snapshot);
                IntPtr popupHandle = popup.Handle;
                Assert(popupHandle != IntPtr.Zero, "popup handle is available for off-screen rendering");
                typeof(UsagePopup).GetMethod("ApplyLayoutMetrics", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, new object[] { false, 420 });
                VScrollBar scroll = Field<VScrollBar>(popup, "bodyScrollBar");
                Assert(Field<bool>(popup, "bodyScrollable") && popup.ClientSize.Height <= 420 &&
                    scroll.Maximum - scroll.LargeChange + 1 > 0,
                    "many reset credits remain within a short working area and enable scrolling");
                Bitmap topView = new Bitmap(popup.ClientSize.Width, popup.ClientSize.Height);
                popup.DrawToBitmap(topView, new Rectangle(Point.Empty, topView.Size));
                Message message = new Message();
                typeof(UsagePopup).GetMethod("ProcessCmdKey", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, new object[] { message, Keys.End });
                int bottom = scroll.Maximum - scroll.LargeChange + 1;
                Assert(scroll.Value == bottom && popup.AccessibleDescription.Contains("Reset credit 20"),
                    "keyboard scrolling reaches the last reset and all entries remain accessible");
                typeof(UsagePopup).GetMethod("OnMouseWheel", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, new object[] { new MouseEventArgs(MouseButtons.None, 0, 0, 0, 120) });
                Assert(scroll.Value < bottom, "mouse wheel scrolls the content back toward the top");
                using (Bitmap scrolled = new Bitmap(popup.ClientSize.Width, popup.ClientSize.Height))
                {
                    popup.DrawToBitmap(scrolled, new Rectangle(Point.Empty, scrolled.Size));
                    for (int y = 4; y < 45; y++)
                        for (int x = 10; x < 330; x++)
                            Assert(topView.GetPixel(x, y) == scrolled.GetPixel(x, y),
                                "scrolling long text cannot paint over the fixed header");
                }
                topView.Dispose();

                MethodInfo layoutInArea = typeof(UsagePopup).GetMethod("ApplyLayoutMetricsInArea", BindingFlags.NonPublic | BindingFlags.Instance);
                Button settingsButton = Field<Button>(popup, "settingsButton");
                Button historyButton = Field<Button>(popup, "historyButton");
                foreach (int logicalWidth in new[] { 240, 320, 400, 520 })
                {
                    int availableWidth = (int)scaleMetric.Invoke(popup, new object[] { logicalWidth });
                    int availableHeight = (int)scaleMetric.Invoke(popup, new object[] { 420 });
                    layoutInArea.Invoke(popup, new object[] { false, availableHeight, availableWidth });
                    Assert(popup.ClientSize.Width <= availableWidth && popup.ClientSize.Height <= availableHeight,
                        "popup remains inside the supplied working area at every supported width");
                    Assert(settingsButton.Left >= 0 && settingsButton.Right <= popup.ClientSize.Width &&
                        historyButton.Left >= 0 && historyButton.Right <= popup.ClientSize.Width &&
                        refresh.Left >= 0 && refresh.Right <= popup.ClientSize.Width &&
                        provider.Left >= 0 && provider.Right <= popup.ClientSize.Width,
                        "header controls remain inside the popup on narrow displays");
                    Assert(!provider.Bounds.IntersectsWith(historyButton.Bounds) &&
                        !historyButton.Bounds.IntersectsWith(refresh.Bounds) &&
                        !refresh.Bounds.IntersectsWith(settingsButton.Bounds),
                        "provider and action buttons never overlap during reflow");
                    if (logicalWidth < 480)
                        Assert(popup.GetUsageRowHeightForText("Next reset: " + extendedText, "Estimated: " + extendedText) > 104,
                            "narrow usage cards stack and expand for their full text");
                    using (Bitmap narrow = new Bitmap(popup.ClientSize.Width, popup.ClientSize.Height))
                        popup.DrawToBitmap(narrow, new Rectangle(Point.Empty, narrow.Size));
                }

                snapshot.StatusMessage = null;
                snapshot.AvailableResets.Clear();
                snapshot.AvailableResetCount = 0;
                popup.UpdateSnapshot(snapshot);
                Assert(!Field<bool>(popup, "bodyScrollable") && scroll.Value == 0,
                    "scrolling is removed when content fits again");
            }
        }

        private static void TestResetEntrySpacing()
        {
            UsageHistoryStore history = new UsageHistoryStore(Path.Combine(Path.GetTempPath(),
                "PopupResetSpacing-" + Guid.NewGuid().ToString("N") + ".json"));
            using (UsagePopup popup = new UsagePopup(new AppSettings(), history))
            {
                UsageSnapshot snapshot = new UsageSnapshot
                {
                    LastUpdated = DateTime.Now,
                    Weekly = new LimitWindow { UsedPercent = 20, WindowMinutes = 10080, ResetAfterSeconds = 86400 },
                    AvailableResetCount = 1
                };
                snapshot.AvailableResets.Add(new RateLimitResetCredit { Title = "Full reset", ExpiresAtUtc = DateTime.UtcNow.AddDays(2) });
                popup.UpdateSnapshot(snapshot);
                int firstHeight = popup.ClientSize.Height;
                snapshot.AvailableResets.Add(new RateLimitResetCredit { Title = "Full reset", ExpiresAtUtc = DateTime.UtcNow.AddDays(3) });
                snapshot.AvailableResetCount = 2;
                popup.UpdateSnapshot(snapshot);
                int secondHeight = popup.ClientSize.Height;
                Font font = Field<Font>(popup, "detailFont");
                int textHeight = TextRenderer.MeasureText("Ag", font, new Size(32767, 32767),
                    TextFormatFlags.SingleLine | TextFormatFlags.NoPadding).Height;
                Assert(secondHeight - firstHeight >= textHeight + 8,
                    "each reset entry has breathing room around its text");

                snapshot.AvailableResetCount = 50;
                popup.UpdateSnapshot(snapshot);
                int withUnitemized = popup.ClientSize.Height;
                Assert(withUnitemized - secondHeight >= textHeight + 8,
                    "the additional resets row receives the same spacing");
                snapshot.AvailableResetCount = 200;
                popup.UpdateSnapshot(snapshot);
                Assert(popup.ClientSize.Height == withUnitemized,
                    "the additional count does not reserve a row per unitemized reset");
            }
        }

        private static void TestObservedDailyUsage()
        {
            DateTime today = DateTime.SpecifyKind(DateTime.Now.Date, DateTimeKind.Local);
            DateTime yesterday = today.AddDays(-1);
            DateTime previousDay = today.AddDays(-2);
            DateTime now = today.AddHours(16).ToUniversalTime();
            DateTime reset = now.AddDays(2), nextReset = reset.AddDays(7);
            List<UsageHistorySample> readings = new List<UsageHistorySample>
            {
                WeeklyReading(previousDay.AddHours(8), 10, reset),
                WeeklyReading(previousDay.AddHours(9), 10, reset),
                WeeklyReading(yesterday.AddHours(9), 20, reset),
                WeeklyReading(yesterday.AddHours(10), 30, reset),
                WeeklyReading(yesterday.AddHours(13), 30, reset), // three-hour gap is unknown
                WeeklyReading(yesterday.AddHours(14), 40, reset),
                WeeklyReading(yesterday.AddHours(15), 5, nextReset), // a reset is not negative usage
                WeeklyReading(yesterday.AddHours(16), 15, nextReset),
                WeeklyReading(today.AddHours(10), 15, nextReset),
                WeeklyReading(today.AddHours(10.5), 16, nextReset) // only half an hour of coverage
            };
            UsagePopup.WeeklyHistorySummary summary = UsagePopup.BuildWeeklyHistorySummary(readings, now);
            Assert(summary.DailyUsage[4].HasValue && Math.Abs(summary.DailyUsage[4].Value) < 0.001,
                "a sufficiently observed inactive day reports zero usage");
            Assert(summary.DailyUsage[5].HasValue && Math.Abs(summary.DailyUsage[5].Value - 30) < 0.001,
                "daily usage sums valid increases while excluding long gaps and reset boundaries");
            Assert(summary.DailyUsage[6].HasValue && Math.Abs(summary.DailyUsage[6].Value - 1) < 0.001 &&
                Math.Abs(summary.DailyCoverageHours[6] - 0.5) < 0.001 && !summary.DailyUsage[3].HasValue,
                "partially observed usage retains its coverage while missing days remain unavailable");

            List<UsageHistorySample> midnight = new List<UsageHistorySample>
            {
                WeeklyReading(previousDay.AddHours(22), 10, reset),
                WeeklyReading(previousDay.AddHours(23), 15, reset),
                WeeklyReading(yesterday, 35, reset), // usage across midnight cannot be assigned reliably
                WeeklyReading(yesterday.AddHours(1), 38, reset)
            };
            UsagePopup.WeeklyHistorySummary split = UsagePopup.BuildWeeklyHistorySummary(midnight, now);
            Assert(split.DailyUsage[4].HasValue && Math.Abs(split.DailyUsage[4].Value - 5) < 0.001 &&
                split.DailyUsage[5].HasValue && Math.Abs(split.DailyUsage[5].Value - 3) < 0.001,
                "intervals crossing midnight are excluded rather than assigned to either day");

            List<UsageHistorySample> multipleCycles = new List<UsageHistorySample>
            {
                WeeklyReading(yesterday.AddHours(8), 0, reset),
                WeeklyReading(yesterday.AddHours(9), 60, reset),
                WeeklyReading(yesterday.AddHours(10), 0, nextReset),
                WeeklyReading(yesterday.AddHours(11), 70, nextReset)
            };
            UsagePopup.WeeklyHistorySummary cycles = UsagePopup.BuildWeeklyHistorySummary(multipleCycles, now);
            Assert(cycles.DailyUsage[5].HasValue && Math.Abs(cycles.DailyUsage[5].Value - 130) < 0.001 &&
                cycles.DailyDates[5].Date == yesterday.Date,
                "multiple observed reset cycles can exceed 100 percent used in the correct local day");
            UsagePopup.WeeklyHistorySummary empty = UsagePopup.BuildWeeklyHistorySummary(new List<UsageHistorySample>(), now);
            for (int day = 0; day < 7; day++)
                Assert(!empty.DailyUsage[day].HasValue && empty.DailyDates[day].Date == today.AddDays(day - 6),
                    "empty history has seven dated, unavailable days");
        }

        private static UsageHistorySample WeeklyReading(DateTime localTime, double used, DateTime reset)
        {
            return new UsageHistorySample
            {
                TimestampUtc = localTime.ToUniversalTime(),
                Weekly = new UsageHistoryWindow { UsedPercent = used, WindowMinutes = 10080, ResetAtUtc = reset }
            };
        }

        private static Bitmap RenderBar(UsagePopup popup)
        {
            Bitmap bitmap = new Bitmap(popup.ClientSize.Width, 100);
            UsageSnapshot snapshot = Field<UsageSnapshot>(popup, "snapshot");
            using (Graphics graphics = Graphics.FromImage(bitmap))
                typeof(UsagePopup).GetMethod("DrawUsageRow", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, new object[] { graphics, "Weekly", snapshot.Weekly, snapshot.LastUpdated,
                        0, 0, Color.White, Color.Gray });
            return bitmap;
        }

        private static T Field<T>(object instance, string name)
        {
            return (T)instance.GetType().GetField(name, BindingFlags.NonPublic | BindingFlags.Instance).GetValue(instance);
        }
        private static void Click(Button button)
        {
            typeof(Button).GetMethod("OnClick", BindingFlags.NonPublic | BindingFlags.Instance)
                .Invoke(button, new object[] { EventArgs.Empty });
        }
        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("Usage popup: " + message);
        }
    }
}
