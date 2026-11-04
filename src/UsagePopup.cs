using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Globalization;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal sealed class UsagePopup : Form
    {
        private const int WmDpiChanged = 0x02E0;
        private const int LogicalWidth = 520;
        private const int LogicalHeaderBottom = 54;
        private const int LogicalHistoryHeight = 116;
        private const int LogicalUsageRowHeight = 104;
        private const int LogicalFooterTopGap = 8;
        private const int LogicalContentBottomPadding = 8;
        private const int LogicalFooterLineHeight = 21;
        private const int LogicalResetEntryPadding = 4;
        private const int LogicalFooterBottomPadding = 10;
        private const string RefreshGlyph = "\uE72C";

        private readonly Button refreshButton;
        private readonly Button settingsButton;
        private readonly Button historyButton;
        private readonly Button providerButton;
        private readonly VScrollBar bodyScrollBar;
        private readonly UsageHistoryStore codexHistory;
        private UsageHistoryStore history;
        private UsageHistoryStore claudeHistory;
        private UsageSnapshot codexSnapshot;
        private UsageSnapshot claudeSnapshot;
        private bool showingClaude;
        private readonly ToolTip actionToolTip;
        private readonly Timer refreshAnimationTimer;
        private readonly Font titleFont;
        private readonly Font rowLabelFont;
        private readonly Font percentFont;
        private readonly Font detailFont;
        private readonly Font resetDetailFont;
        private readonly Font detailHeadingFont;
        private readonly Font glyphFont;
        private readonly float textScale;

        private AppSettings settings;
        private UsageSnapshot snapshot;
        private int currentDpi = 96;
        private int refreshAnimationAngle = -90;
        private bool refreshing;
        private bool claudeRefreshing;
        private bool updatingLayout;
        private bool bodyScrollable;
        private int layoutWidth;
        private WeeklyHistorySummary historySummary;
        private int hoveredHistoryDay = -1;
#if WPF_UI
        private System.Windows.Forms.Integration.ElementHost wpfHost;
        private UsagePopupView wpfView;
#endif

        internal sealed class WeeklyHistorySummary
        {
            public readonly DateTime[] DailyDates = new DateTime[7];
            public readonly double?[] DailyUsage = new double?[7];
            public readonly double[] DailyCoverageHours = new double[7];
            public readonly bool[] DailyPartial = new bool[7];
        }

        public event EventHandler RefreshRequested;
        public event EventHandler SettingsRequested;
        public event EventHandler HistoryRequested;

        public bool IsShowingClaude { get { return showingClaude; } }

        public UsagePopup(AppSettings settings, UsageHistoryStore history)
        {
            this.settings = settings;
            this.history = history;
            codexHistory = history;
            textScale = GetTextScale(SystemFonts.MessageBoxFont.SizeInPoints);
            layoutWidth = ScaleMetric(LogicalWidth);

            titleFont = new Font("Segoe UI", 12.0f * textScale, FontStyle.Bold, GraphicsUnit.Point);
            rowLabelFont = new Font("Segoe UI", 12.0f * textScale, FontStyle.Bold, GraphicsUnit.Point);
            percentFont = new Font("Segoe UI", 9.0f * textScale, FontStyle.Bold, GraphicsUnit.Point);
            detailFont = new Font("Segoe UI", 10.0f * textScale, FontStyle.Regular, GraphicsUnit.Point);
            resetDetailFont = new Font("Segoe UI", 9.0f * textScale, FontStyle.Regular, GraphicsUnit.Point);
            detailHeadingFont = new Font("Segoe UI", 10.5f * textScale, FontStyle.Bold, GraphicsUnit.Point);
            glyphFont = new Font("Segoe MDL2 Assets", 12.0f * textScale, FontStyle.Regular, GraphicsUnit.Point);

            refreshAnimationTimer = new Timer();
            refreshAnimationTimer.Interval = 75;
            refreshAnimationTimer.Tick += RefreshAnimationTimer_Tick;

            Text = "Codex Usage";
            FormBorderStyle = FormBorderStyle.None;
            ShowInTaskbar = false;
            TopMost = true;
            StartPosition = FormStartPosition.Manual;
            AutoScaleDimensions = new SizeF(96.0f, 96.0f);
            AutoScaleMode = AutoScaleMode.Dpi;
            KeyPreview = true;
            DoubleBuffered = true;
            Padding = new Padding(0);

            refreshButton = CreateActionButton("refreshButton", RefreshGlyph, "Refresh usage", 0);
            settingsButton = CreateActionButton("settingsButton", "\uE713", "Open settings", 1);
            historyButton = CreateActionButton("historyButton", "", "Open usage history", 2);
            historyButton.Paint += HistoryButton_Paint;
            providerButton = CreateActionButton("providerButton", "", "Switch usage provider", 3);
            providerButton.Font = titleFont;
            providerButton.Visible = false;
            providerButton.Click += delegate
            {
                if (claudeSnapshot == null) return;
                showingClaude = !showingClaude;
                UpdateSelectedProvider();
            };
            refreshButton.Click += RefreshButton_Click;
            refreshButton.Paint += RefreshButton_Paint;
            settingsButton.Click += SettingsButton_Click;
            historyButton.Click += delegate
            {
                EventHandler handler = HistoryRequested;
                if (handler != null) handler(this, EventArgs.Empty);
            };
            Controls.Add(refreshButton);
            Controls.Add(settingsButton);
            Controls.Add(historyButton);
            Controls.Add(providerButton);
            bodyScrollBar = new VScrollBar();
            bodyScrollBar.Visible = false;
            bodyScrollBar.AccessibleName = "Scroll popup content";
            bodyScrollBar.AccessibleDescription = "Scroll to see all usage and reset details";
            bodyScrollBar.TabIndex = 4;
            bodyScrollBar.TabStop = true;
            bodyScrollBar.ValueChanged += delegate { hoveredHistoryDay = -1; Invalidate(); };
            Controls.Add(bodyScrollBar);

            actionToolTip = new ToolTip();
            actionToolTip.AutomaticDelay = 350;
            actionToolTip.AutoPopDelay = 5000;
            actionToolTip.ShowAlways = true;
            actionToolTip.SetToolTip(refreshButton, "Refresh usage");
            actionToolTip.SetToolTip(settingsButton, "Open settings");
            actionToolTip.SetToolTip(historyButton, "Open usage history");

#if WPF_UI
            InitializeWpfPopup();
#endif
            ApplyThemeColors();
            UpdateActionButtonState();
            UpdateLayoutMetrics(false);
        }

        public void ApplySettings(AppSettings value)
        {
            settings = value;
            ApplyThemeColors();
            UpdateLayoutMetrics(true);
            Invalidate(true);
        }

        public void UpdateSnapshot(UsageSnapshot value)
        {
            codexSnapshot = value;
            UpdateSelectedProvider();
        }

        public void UpdateClaudeSnapshot(UsageSnapshot value, UsageHistoryStore providerHistory)
        {
            claudeSnapshot = value != null && value.HasPrimaryLimit && providerHistory != null ? value : null;
            claudeHistory = providerHistory;
            if (claudeSnapshot == null) showingClaude = false;
            UpdateSelectedProvider();
        }

        private void UpdateSelectedProvider()
        {
            snapshot = showingClaude ? claudeSnapshot : codexSnapshot;
            history = showingClaude ? claudeHistory : codexHistory;
            RefreshHistorySummary();
            Text = showingClaude ? "Claude Usage" : "Codex Usage";
            providerButton.Visible = claudeSnapshot != null;
            providerButton.Text = (showingClaude ? "Claude" : "Codex") + " \u21c4";
            providerButton.AccessibleName = "Showing " + (showingClaude ? "Claude" : "Codex") +
                " usage; switch to " + (showingClaude ? "Codex" : "Claude");
            actionToolTip.SetToolTip(providerButton, "Switch to " + (showingClaude ? "Codex" : "Claude") + " usage");
            actionToolTip.SetToolTip(historyButton, "Open " + (showingClaude ? "Claude" : "Codex") + " usage history");
            AccessibleName = Text;
            AccessibleDescription = BuildAccessibleDetails();
            UpdateActionButtonState();
            UpdateLayoutMetrics(true);
            Invalidate(true);
        }

        public void SetRefreshing(bool value)
        {
            if (refreshing == value)
            {
                return;
            }

            refreshing = value;
            UpdateActionButtonState();
            UpdateLayoutMetrics(true);
            Invalidate(true);
        }

        public void SetClaudeRefreshing(bool value)
        {
            if (claudeRefreshing == value) return;
            claudeRefreshing = value;
            UpdateActionButtonState();
            UpdateLayoutMetrics(true);
            Invalidate(true);
        }

        public void ShowNear(Point cursor)
        {
            RefreshHistorySummary();
            Rectangle area = Screen.FromPoint(cursor).WorkingArea;
            ApplyLayoutMetricsInArea(false, area.Height - ScaleMetric(16), area.Width - ScaleMetric(16));
            int x = cursor.X - Width + ScaleMetric(28);
            int y = cursor.Y - Height - ScaleMetric(18);
            int edgeMargin = ScaleMetric(8);

            if (x < area.Left)
            {
                x = area.Left + edgeMargin;
            }
            if (y < area.Top)
            {
                y = cursor.Y + ScaleMetric(18);
            }
            if (x + Width > area.Right)
            {
                x = area.Right - Width - edgeMargin;
            }
            if (y + Height > area.Bottom)
            {
                y = area.Bottom - Height - edgeMargin;
            }

            Location = new Point(x, y);
            Show();
            Activate();
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                refreshAnimationTimer.Dispose();
                actionToolTip.Dispose();
                titleFont.Dispose();
                rowLabelFont.Dispose();
                percentFont.Dispose();
                detailFont.Dispose();
                resetDetailFont.Dispose();
                detailHeadingFont.Dispose();
                glyphFont.Dispose();
            }

            base.Dispose(disposing);
        }

        protected override void OnHandleCreated(EventArgs e)
        {
            base.OnHandleCreated(e);

            using (Graphics graphics = CreateGraphics())
            {
                int dpi = (int)Math.Round(graphics.DpiX);
                if (dpi > 0)
                {
                    currentDpi = dpi;
                }
            }

            UpdateLayoutMetrics(false);
        }

        protected override void OnDeactivate(EventArgs e)
        {
            base.OnDeactivate(e);
            Hide();
        }

        protected override void OnFormClosing(FormClosingEventArgs e)
        {
            if (e.CloseReason == CloseReason.UserClosing)
            {
                e.Cancel = true;
                Hide();
                return;
            }

            base.OnFormClosing(e);
        }

        protected override bool ProcessCmdKey(ref Message msg, Keys keyData)
        {
            if (keyData == Keys.Escape)
            {
                Hide();
                return true;
            }

            if (bodyScrollable)
            {
                if (keyData == Keys.Down) { ScrollBody(bodyScrollBar.SmallChange); return true; }
                if (keyData == Keys.Up) { ScrollBody(-bodyScrollBar.SmallChange); return true; }
                if (keyData == Keys.PageDown) { ScrollBody(bodyScrollBar.LargeChange); return true; }
                if (keyData == Keys.PageUp) { ScrollBody(-bodyScrollBar.LargeChange); return true; }
                if (keyData == Keys.Home) { bodyScrollBar.Value = bodyScrollBar.Minimum; return true; }
                if (keyData == Keys.End) { bodyScrollBar.Value = GetMaximumScrollValue(); return true; }
            }

            return base.ProcessCmdKey(ref msg, keyData);
        }

        protected override void OnMouseWheel(MouseEventArgs e)
        {
            base.OnMouseWheel(e);
            if (bodyScrollable && e.Delta != 0)
                ScrollBody(-(e.Delta / SystemInformation.MouseWheelScrollDelta) * bodyScrollBar.SmallChange * 3);
        }

        protected override void OnMouseMove(MouseEventArgs e)
        {
            base.OnMouseMove(e);
#if !WPF_UI
            int day = GetHistoryDayAt(e.Location);
            if (hoveredHistoryDay != day)
            {
                hoveredHistoryDay = day;
                Invalidate();
            }
#endif
        }

        protected override void OnMouseLeave(EventArgs e)
        {
            base.OnMouseLeave(e);
            if (hoveredHistoryDay >= 0)
            {
                hoveredHistoryDay = -1;
                Invalidate();
            }
        }

        private int GetMaximumScrollValue()
        {
            return Math.Max(bodyScrollBar.Minimum, bodyScrollBar.Maximum - bodyScrollBar.LargeChange + 1);
        }

        private void ScrollBody(int delta)
        {
            bodyScrollBar.Value = Math.Max(bodyScrollBar.Minimum,
                Math.Min(GetMaximumScrollValue(), bodyScrollBar.Value + delta));
        }

        protected override void OnResize(EventArgs e)
        {
            base.OnResize(e);
            UpdateRegion();
        }

        protected override void OnSystemColorsChanged(EventArgs e)
        {
            base.OnSystemColorsChanged(e);
            ApplyThemeColors();
#if WPF_UI
            UpdateLayoutMetrics(true);
#endif
            Invalidate(true);
        }

        protected override void WndProc(ref Message m)
        {
            bool dpiChanged = false;
            if (m.Msg == WmDpiChanged)
            {
                int dpi = (int)(m.WParam.ToInt64() & 0xFFFF);
                if (dpi > 0 && dpi != currentDpi)
                {
                    currentDpi = dpi;
                    dpiChanged = true;
                }
            }

            base.WndProc(ref m);

            if (dpiChanged)
            {
                UpdateLayoutMetrics(true);
                Invalidate(true);
            }
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            base.OnPaint(e);

            Graphics graphics = e.Graphics;
            graphics.SmoothingMode = SmoothingMode.AntiAlias;
            graphics.CompositingQuality = CompositingQuality.HighQuality;
            graphics.PixelOffsetMode = PixelOffsetMode.HighQuality;

            bool dark = IsDarkTheme();
            Color backColor = GetBackColor(dark);
            Color textColor = GetTextColor(dark);
            Color mutedColor = GetMutedColor(dark);
            Color borderColor = GetBorderColor(dark);

            graphics.Clear(backColor);

            DrawHeader(graphics, textColor, mutedColor);

            if (bodyScrollable)
            {
                int bodyTop = ScaleMetric(GetLogicalHeaderBottom());
                int height = Math.Max(1, ClientSize.Height - bodyTop - ScaleMetric(4));
                GraphicsState state = graphics.Save();
                graphics.SetClip(new Rectangle(0, bodyTop, BodyWidth, height), CombineMode.Intersect);
                DrawBody(graphics, textColor, mutedColor, borderColor, dark, bodyScrollBar.Value);
                graphics.Restore(state);
            }
            else DrawBody(graphics, textColor, mutedColor, borderColor, dark, 0);

            using (Pen borderPen = new Pen(borderColor, Math.Max(1.0f, currentDpi / 96.0f)))
            using (GraphicsPath outline = RoundedRectangle(
                new Rectangle(0, 0, ClientSize.Width, ClientSize.Height), ScaleMetric(8)))
                graphics.DrawPath(borderPen, outline);
        }

        private void DrawBody(Graphics graphics, Color textColor, Color mutedColor, Color borderColor, bool dark, int offset)
        {
            DateTime lastUpdated = snapshot != null ? snapshot.LastUpdated : DateTime.MinValue;
            int rowY = GetLogicalHeaderBottom();
            if (HasWeeklyHistorySummary())
            {
                DrawHistorySummary(graphics, ScaleMetric(rowY) - offset, textColor, mutedColor, dark);
                rowY += GetLogicalHistoryHeight();
            }
            if (snapshot != null && snapshot.Weekly != null)
            {
                DrawUsageRow(
                    graphics,
                    "Weekly",
                    snapshot.Weekly,
                    lastUpdated,
                    ScaleMetric(12),
                    ScaleMetric(rowY) - offset,
                    textColor,
                    mutedColor);
                rowY += GetUsageRowHeight(snapshot.Weekly);
            }
            if (snapshot != null && snapshot.FiveHour != null)
            {
                DrawUsageRow(
                    graphics,
                    "5-hour",
                    snapshot.FiveHour,
                    lastUpdated,
                    ScaleMetric(12),
                    ScaleMetric(rowY) - offset,
                    textColor,
                    mutedColor);
            }

            if (!string.IsNullOrEmpty(BuildStatusLine()) || HasResetDetails())
            {
                DrawFooter(graphics, textColor, mutedColor, borderColor, dark, offset);
            }
        }

        private Button CreateActionButton(string name, string glyph, string accessibleName, int tabIndex)
        {
            Button button = new Button();
            button.Name = name;
            button.Text = glyph;
            button.Font = glyphFont;
            button.AccessibleName = accessibleName;
            button.AccessibleDescription = accessibleName;
            button.AccessibleRole = AccessibleRole.PushButton;
            button.TabIndex = tabIndex;
            button.TabStop = true;
            button.AutoSize = false;
            button.Margin = new Padding(0);
            button.Padding = new Padding(0);
            button.FlatStyle = FlatStyle.Flat;
            button.FlatAppearance.BorderSize = 0;
            button.UseVisualStyleBackColor = false;
            button.UseCompatibleTextRendering = false;
            button.TextAlign = ContentAlignment.MiddleCenter;
            button.Cursor = Cursors.Hand;
            return button;
        }

        private void RefreshButton_Click(object sender, EventArgs e)
        {
            if (IsRefreshActive())
            {
                return;
            }

            EventHandler handler = RefreshRequested;
            if (handler != null)
            {
                handler(this, EventArgs.Empty);
            }
        }

        private void RefreshAnimationTimer_Tick(object sender, EventArgs e)
        {
            refreshAnimationAngle = (refreshAnimationAngle + 30) % 360;
            refreshButton.Invalidate();
        }

        private void RefreshButton_Paint(object sender, PaintEventArgs e)
        {
            if (!IsRefreshActive())
            {
                return;
            }

            float scale = (currentDpi / 96.0f) * textScale;
            float diameter = 14.0f * scale;
            float penWidth = Math.Max(1.5f, 1.8f * scale);
            RectangleF bounds = new RectangleF(
                (refreshButton.ClientSize.Width - diameter) / 2.0f,
                (refreshButton.ClientSize.Height - diameter) / 2.0f,
                diameter,
                diameter);
            Color spinnerColor = SystemInformation.HighContrast
                ? SystemColors.ControlText
                : refreshButton.ForeColor;

            e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
            e.Graphics.PixelOffsetMode = PixelOffsetMode.HighQuality;
            using (Pen pen = new Pen(spinnerColor, penWidth))
            {
                pen.StartCap = LineCap.Round;
                pen.EndCap = LineCap.Round;
                e.Graphics.DrawArc(pen, bounds, refreshAnimationAngle, 265);
            }
        }

        private void SettingsButton_Click(object sender, EventArgs e)
        {
            EventHandler handler = SettingsRequested;
            if (handler != null)
            {
                handler(this, EventArgs.Empty);
            }
        }

        private void HistoryButton_Paint(object sender, PaintEventArgs e)
        {
            float scale = (currentDpi / 96.0f) * textScale;
            float left = (historyButton.Width - 16 * scale) / 2;
            float top = (historyButton.Height - 16 * scale) / 2;
            e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
            Color color = SystemInformation.HighContrast ? historyButton.ForeColor :
                (IsDarkTheme() ? Color.FromArgb(100, 187, 255) : Color.FromArgb(0, 105, 185));
            using (Pen pen = new Pen(color, Math.Max(2, 2.2f * scale)))
            {
                pen.StartCap = LineCap.Round;
                pen.EndCap = LineCap.Round;
                e.Graphics.DrawLine(pen, left + 3 * scale, top + 11 * scale, left + 3 * scale, top + 15 * scale);
                e.Graphics.DrawLine(pen, left + 8 * scale, top + 7 * scale, left + 8 * scale, top + 15 * scale);
                e.Graphics.DrawLine(pen, left + 13 * scale, top + 1 * scale, left + 13 * scale, top + 15 * scale);
            }
        }

        private string GetHeaderDetail()
        {
            if (IsRefreshActive()) return "Refreshing...";
            return (settings == null || settings.ShowPopupLastUpdated) && snapshot != null &&
                snapshot.LastUpdated != DateTime.MinValue ? "Updated " + TimeFormatter.FormatClock(snapshot.LastUpdated) : null;
        }

        private int GetHeaderDetailLineCount()
        {
            string detail = GetHeaderDetail();
            return string.IsNullOrEmpty(detail) ? 0 : GetWrappedLineCount(detail, Math.Max(1, layoutWidth - ScaleMetric(28)));
        }

        private bool UseNarrowLayout { get { return LogicalLayoutWidth < 480; } }
        private bool SeparateProviderRow { get { return UseNarrowLayout && claudeSnapshot != null && LogicalLayoutWidth < 260; } }

        private int GetLogicalHeaderBottom()
        {
            return !UseNarrowLayout ? LogicalHeaderBottom :
                32 + GetHeaderDetailLineCount() * 21 + 7 + 36 + 8 + (SeparateProviderRow ? 43 : 0);
        }

        private void DrawHeader(Graphics graphics, Color textColor, Color mutedColor)
        {
            if (UseNarrowLayout)
            {
                string narrowTitle = showingClaude ? "Claude Usage" : "Codex Usage";
                TextRenderer.DrawText(graphics, narrowTitle, titleFont,
                    new Rectangle(ScaleMetric(14), ScaleMetric(6), Math.Max(0, ClientSize.Width - ScaleMetric(28)), ScaleMetric(24)),
                    textColor, TextFormatFlags.SingleLine | TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding);
                string detail = GetHeaderDetail();
                if (!string.IsNullOrEmpty(detail))
                    TextRenderer.DrawText(graphics, detail, detailFont,
                        new Rectangle(ScaleMetric(14), ScaleMetric(32), Math.Max(0, ClientSize.Width - ScaleMetric(28)),
                            ScaleMetric(GetHeaderDetailLineCount() * 21)), mutedColor,
                        TextFormatFlags.WordBreak | TextFormatFlags.TextBoxControl | TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding);
                return;
            }
            int left = ScaleMetric(14);
            int top = ScaleMetric(14);
            int right = historyButton.Left - ScaleMetric(8);
            int height = ScaleMetric(24);
            Rectangle titleBounds = new Rectangle(left, top, Math.Max(0, right - left), height);
            TextFormatFlags flags = TextFormatFlags.SingleLine |
                TextFormatFlags.VerticalCenter |
                TextFormatFlags.NoPrefix |
                TextFormatFlags.NoPadding;

            string title = showingClaude ? "Claude Usage" : "Codex Usage";
            Size titleSize = TextRenderer.MeasureText(
                graphics,
                title,
                titleFont,
                new Size(32767, height),
                flags);
            Rectangle titleTextBounds = new Rectangle(
                titleBounds.Left,
                titleBounds.Top,
                Math.Min(titleBounds.Width, titleSize.Width),
                titleBounds.Height);
            if (claudeSnapshot == null)
                TextRenderer.DrawText(graphics, title, titleFont, titleTextBounds, textColor, flags);
            else
                titleTextBounds.Width = ScaleMetric(100);

            string headerDetail = GetHeaderDetail();

            if (!string.IsNullOrEmpty(headerDetail))
            {
                int detailLeft = titleTextBounds.Right + ScaleMetric(8);
                Rectangle detailBounds = new Rectangle(
                    detailLeft,
                    top,
                    Math.Max(0, right - detailLeft),
                    height);
                TextRenderer.DrawText(
                    graphics,
                    headerDetail,
                    detailFont,
                    detailBounds,
                    mutedColor,
                    flags | TextFormatFlags.EndEllipsis);
            }
        }

        private bool HasWeeklyHistorySummary()
        {
            return snapshot != null && snapshot.Weekly != null && history != null;
        }

        private string GetHistorySummaryDetail()
        {
            if (historySummary != null && hoveredHistoryDay >= 0 && hoveredHistoryDay < historySummary.DailyUsage.Length)
                return FormatDailyUsageDetail(historySummary.DailyDates[hoveredHistoryDay],
                    historySummary.DailyUsage[hoveredHistoryDay], historySummary.DailyPartial[hoveredHistoryDay]);
            return "Hover over a day for details";
        }

        private int GetLogicalHistoryHeight()
        {
            return UseNarrowLayout ? LogicalHistoryHeight + 21 : LogicalHistoryHeight;
        }

        internal static string FormatDailyUsageDetail(DateTime date, double? daily, bool partial)
        {
            string usage = "Usage unavailable";
            if (daily.HasValue)
            {
                string value = daily.Value > 0 && daily.Value < 0.001 ? "<0.001" :
                    daily.Value.ToString("0.###", CultureInfo.CurrentCulture);
                usage = "Observed: " + value + "% used" + (partial ? " · Partial coverage" : "");
            }
            return date.ToString("ddd, MMM d", CultureInfo.CurrentCulture) + " · " + usage;
        }

        private double GetMaxObservedDailyUsage()
        {
            double maximum = 1;
            if (historySummary != null)
                for (int day = 0; day < historySummary.DailyUsage.Length; day++)
                    if (historySummary.DailyUsage[day].HasValue)
                        maximum = Math.Max(maximum, historySummary.DailyUsage[day].Value);
            return maximum;
        }

        private void RefreshHistorySummary()
        {
            historySummary = HasWeeklyHistorySummary()
                ? BuildWeeklyHistorySummary(history.Samples, DateTime.UtcNow) : null;
        }

        internal static WeeklyHistorySummary BuildWeeklyHistorySummary(List<UsageHistorySample> samples, DateTime nowUtc)
        {
            WeeklyHistorySummary summary = new WeeklyHistorySummary();
            nowUtc = nowUtc.ToUniversalTime();
            DateTime firstLocalDay = nowUtc.ToLocalTime().Date.AddDays(-6);
            for (int day = 0; day < 7; day++)
            {
                summary.DailyDates[day] = firstLocalDay.AddDays(day);
                summary.DailyPartial[day] = true;
            }
            if (samples == null || samples.Count < 2) return summary;
            DateTime firstUtc = DateTime.SpecifyKind(firstLocalDay, DateTimeKind.Local).ToUniversalTime();
            double[] dailyUsage = new double[7];
            for (int i = 1; i < samples.Count; i++)
            {
                UsageHistorySample before = samples[i - 1];
                UsageHistorySample after = samples[i];
                if (before == null || after == null || before.Weekly == null || after.Weekly == null ||
                    before.Weekly.WindowMinutes != after.Weekly.WindowMinutes ||
                    UsageHistoryStore.IsReset(before, after, true)) continue;
                double elapsed = (after.TimestampUtc - before.TimestampUtc).TotalHours;
                if (elapsed <= 0 || elapsed > 2 || after.TimestampUtc <= firstUtc || before.TimestampUtc >= nowUtc) continue;
                // The timing of an increase across midnight is unknown. Only
                // credit intervals wholly within one local day and before now.
                DateTime beforeDate = before.TimestampUtc.ToLocalTime().Date;
                DateTime afterDate = after.TimestampUtc.ToLocalTime().Date;
                int observedDay = (beforeDate - firstLocalDay).Days;
                if (after.TimestampUtc <= nowUtc && beforeDate == afterDate && observedDay >= 0 && observedDay < 7 &&
                    !double.IsNaN(before.Weekly.UsedPercent) && !double.IsInfinity(before.Weekly.UsedPercent) &&
                    !double.IsNaN(after.Weekly.UsedPercent) && !double.IsInfinity(after.Weekly.UsedPercent) &&
                    before.Weekly.UsedPercent >= 0 && before.Weekly.UsedPercent <= 100 &&
                    after.Weekly.UsedPercent >= 0 && after.Weekly.UsedPercent <= 100)
                {
                    dailyUsage[observedDay] += Math.Max(0, after.Weekly.UsedPercent - before.Weekly.UsedPercent);
                    summary.DailyCoverageHours[observedDay] += elapsed;
                }
            }
            for (int day = 0; day < 7; day++)
            {
                if (summary.DailyCoverageHours[day] > 0)
                {
                    summary.DailyUsage[day] = dailyUsage[day];
                    DateTime start = DateTime.SpecifyKind(summary.DailyDates[day], DateTimeKind.Local).ToUniversalTime();
                    DateTime end = DateTime.SpecifyKind(summary.DailyDates[day].AddDays(1), DateTimeKind.Local).ToUniversalTime();
                    if (end > nowUtc) end = nowUtc;
                    summary.DailyPartial[day] = summary.DailyCoverageHours[day] + 0.000001 < (end - start).TotalHours;
                }
            }
            return summary;
        }

        private void DrawHistorySummary(Graphics graphics, int y, Color textColor, Color mutedColor, bool dark)
        {
            int cardWidth = Math.Max(1, BodyWidth - ScaleMetric(24));
            Rectangle card = new Rectangle(ScaleMetric(12), y, cardWidth, ScaleMetric(GetLogicalHistoryHeight() - 10));
            using (GraphicsPath path = RoundedRectangle(card, ScaleMetric(7)))
            using (Brush brush = new SolidBrush(GetCardColor(dark)))
            using (Pen pen = new Pen(GetBorderColor(dark), Math.Max(1.0f, currentDpi / 96.0f)))
            {
                graphics.FillPath(brush, path);
                graphics.DrawPath(pen, path);
            }
            TextFormatFlags flags = TextFormatFlags.SingleLine | TextFormatFlags.VerticalCenter |
                TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding | TextFormatFlags.EndEllipsis |
                TextFormatFlags.PreserveGraphicsClipping;
            int left = card.Left + ScaleMetric(10);
            int innerWidth = Math.Max(1, card.Width - ScaleMetric(20));
            TextRenderer.DrawText(graphics, "Usage history", detailHeadingFont,
                new Rectangle(left, y + ScaleMetric(7), innerWidth, ScaleMetric(20)), textColor, flags);
            TextRenderer.DrawText(graphics, "Last 7 days", detailFont,
                new Rectangle(left, y + ScaleMetric(7), innerWidth, ScaleMetric(20)), mutedColor, flags | TextFormatFlags.Right);

            Rectangle chart = GetHistoryChartBounds(y);
            int bottom = y + ScaleMetric(58);
            for (int day = 0; day < 7; day++)
            {
                int start = chart.Left + chart.Width * day / 7;
                int end = chart.Left + chart.Width * (day + 1) / 7;
                int barWidth = Math.Max(1, Math.Min(ScaleMetric(20), end - start - ScaleMetric(4)));
                int x = start + (end - start - barWidth) / 2;
                double? observed = historySummary != null ? historySummary.DailyUsage[day] : null;
                if (observed.HasValue)
                {
                    int height = Math.Max(ScaleMetric(2), (int)Math.Round(ScaleMetric(28) *
                        Math.Max(0, observed.Value) / GetMaxObservedDailyUsage()));
                    Color color = SystemInformation.HighContrast ? SystemColors.Highlight : hoveredHistoryDay == day ?
                        Color.FromArgb(131, 205, 255) :
                        (day == 6 ? Color.FromArgb(71, 172, 247) : Color.FromArgb(54, 112, 172));
                    Rectangle bar = new Rectangle(x, bottom - height, barWidth, height);
                    using (GraphicsPath path = RoundedRectangle(bar, Math.Min(ScaleMetric(2), height / 2)))
                    using (Brush brush = new SolidBrush(color)) graphics.FillPath(brush, path);
                }
                else
                {
                    Color marker = hoveredHistoryDay == day
                        ? (SystemInformation.HighContrast ? SystemColors.Highlight : Color.FromArgb(131, 205, 255))
                        : GetBorderColor(dark);
                    using (Pen pen = new Pen(marker, Math.Max(1.0f, currentDpi / 96.0f)))
                        graphics.DrawLine(pen, x, bottom, x + barWidth, bottom);
                }
                if (historySummary != null)
                    TextRenderer.DrawText(graphics, historySummary.DailyDates[day].ToString("ddd", CultureInfo.CurrentCulture),
                        resetDetailFont, new Rectangle(start, y + ScaleMetric(60), end - start, ScaleMetric(16)),
                        mutedColor, flags | TextFormatFlags.HorizontalCenter);
            }
            TextFormatFlags detailFlags = UseNarrowLayout
                ? TextFormatFlags.WordBreak | TextFormatFlags.TextBoxControl | TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding | TextFormatFlags.PreserveGraphicsClipping
                : flags;
            TextRenderer.DrawText(graphics, GetHistorySummaryDetail(), detailFont,
                new Rectangle(left, y + ScaleMetric(79), innerWidth, ScaleMetric(UseNarrowLayout ? 40 : 20)), mutedColor, detailFlags);
        }

        private Rectangle GetHistoryChartBounds(int y)
        {
            return new Rectangle(ScaleMetric(22), y + ScaleMetric(28),
                Math.Max(1, BodyWidth - ScaleMetric(44)), ScaleMetric(48));
        }

        private int GetHistoryDayAt(Point point)
        {
            if (!HasWeeklyHistorySummary() || historySummary == null) return -1;
            int bodyTop = ScaleMetric(GetLogicalHeaderBottom());
            if (point.Y < bodyTop || point.Y >= ClientSize.Height - ScaleMetric(4)) return -1;
            int y = bodyTop - (bodyScrollable ? bodyScrollBar.Value : 0);
            Rectangle chart = GetHistoryChartBounds(y);
            if (!chart.Contains(point)) return -1;
            for (int day = 0; day < 7; day++)
                if (point.X < chart.Left + (long)chart.Width * (day + 1) / 7) return day;
            return -1;
        }

        private string GetResetDetail(LimitWindow window, DateTime lastUpdated)
        {
            if (settings != null && !settings.ShowPopupResetTimes) return "";
            return window.ResetAfterSeconds.HasValue
                ? "Next reset: " + TimeFormatter.FormatResetDateTime(lastUpdated, window.ResetAfterSeconds.Value) +
                    " (" + TimeFormatter.FormatDuration(window.ResetAfterSeconds.Value) + ")"
                : "Next reset: ?";
        }

        private int GetUsageRowHeight(LimitWindow window)
        {
            bool weekly = snapshot != null && window == snapshot.Weekly;
            string forecast = history.GetForecast(weekly, snapshot, DateTime.UtcNow);
            return GetUsageRowHeightForText(GetResetDetail(window, snapshot.LastUpdated), forecast);
        }

        internal int GetUsageRowHeightForText(string resetDetail, string forecast)
        {
            // Reserve scrollbar width even when it is currently hidden so the
            // layout does not change when the popup becomes scrollable.
            int width = Math.Max(1, layoutWidth - ScaleMetric(UseNarrowLayout ? 68 : 253));
            int lines = (string.IsNullOrEmpty(resetDetail) ? 0 : GetWrappedLineCount(resetDetail, width)) +
                (string.IsNullOrEmpty(forecast) ? 0 : GetWrappedLineCount(forecast, width));
            return Math.Max(LogicalUsageRowHeight, (UseNarrowLayout ? 54 : 8) + lines * 21 + 9 + 31 + 10);
        }

        private void DrawUsageRow(
            Graphics graphics,
            string label,
            LimitWindow window,
            DateTime lastUpdated,
            int x,
            int y,
            Color textColor,
            Color mutedColor)
        {
            if (window == null)
            {
                return;
            }

            int width = Math.Max(1, BodyWidth - x - ScaleMetric(12));
            bool dark = IsDarkTheme();
            string resetDetail = GetResetDetail(window, lastUpdated);
            bool weekly = window == snapshot.Weekly;
            string forecast = history.GetForecast(weekly, snapshot, DateTime.UtcNow);
            int rowHeight = GetUsageRowHeightForText(resetDetail, forecast);
            Rectangle card = new Rectangle(x, y, width, ScaleMetric(rowHeight - 10));
            Color cardColor = GetCardColor(dark);
            using (GraphicsPath path = RoundedRectangle(card, ScaleMetric(7)))
            using (Brush brush = new SolidBrush(cardColor))
            using (Pen pen = new Pen(GetBorderColor(dark), Math.Max(1.0f, currentDpi / 96.0f)))
            {
                graphics.FillPath(brush, path);
                graphics.DrawPath(pen, path);
            }

            TextFormatFlags singleLine = TextFormatFlags.SingleLine |
                TextFormatFlags.VerticalCenter |
                TextFormatFlags.NoPrefix |
                TextFormatFlags.NoPadding | TextFormatFlags.EndEllipsis | TextFormatFlags.PreserveGraphicsClipping;
            int textLeft = x + ScaleMetric(54);
            int detailLeft = x + ScaleMetric(UseNarrowLayout ? 10 : 195);
            int detailWidth = Math.Max(0, card.Right - ScaleMetric(10) - detailLeft);
            int resetLines = string.IsNullOrEmpty(resetDetail) ? 0 : GetWrappedLineCount(resetDetail, detailWidth);
            int forecastLines = string.IsNullOrEmpty(forecast) ? 0 : GetWrappedLineCount(forecast, detailWidth);
            int remaining = window.RemainingPercent;
            DrawUsageIcon(graphics, new Rectangle(x + ScaleMetric(10), y + ScaleMetric(10),
                ScaleMetric(34), ScaleMetric(34)), label == "Weekly", dark);
            int labelWidth = UseNarrowLayout ? Math.Max(0, card.Right - ScaleMetric(10) - textLeft) :
                Math.Max(0, detailLeft - textLeft - ScaleMetric(8));
            TextRenderer.DrawText(graphics, label, rowLabelFont,
                new Rectangle(textLeft, y + ScaleMetric(17), labelWidth, ScaleMetric(20)),
                textColor, singleLine);
            if (!UseNarrowLayout)
                using (Pen divider = new Pen(GetBorderColor(dark), Math.Max(1.0f, currentDpi / 96.0f)))
                    graphics.DrawLine(divider, detailLeft - ScaleMetric(9), y + ScaleMetric(10),
                        detailLeft - ScaleMetric(9), y + ScaleMetric(Math.Max(45, 5 + (resetLines + forecastLines) * 21)));
            TextFormatFlags wrapped = TextFormatFlags.WordBreak | TextFormatFlags.TextBoxControl |
                TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding | TextFormatFlags.PreserveGraphicsClipping;
            int detailTop = UseNarrowLayout ? 54 : 8;
            if (!string.IsNullOrEmpty(resetDetail))
                TextRenderer.DrawText(graphics, resetDetail, detailFont,
                    new Rectangle(detailLeft, y + ScaleMetric(detailTop), detailWidth, ScaleMetric(resetLines * 21)), mutedColor, wrapped);
            if (!string.IsNullOrEmpty(forecast))
                TextRenderer.DrawText(graphics, forecast, detailFont,
                    new Rectangle(detailLeft, y + ScaleMetric(detailTop + resetLines * 21), detailWidth,
                        ScaleMetric(forecastLines * 21)), mutedColor, wrapped);

            Rectangle barBounds = new Rectangle(x + ScaleMetric(10), y + ScaleMetric(rowHeight - 41),
                width - ScaleMetric(20), ScaleMetric(18));
            Color trackColor = GetTrackColor(dark);
            using (GraphicsPath track = RoundedRectangle(barBounds, ScaleMetric(5)))
            using (Brush trackBrush = new SolidBrush(trackColor))
            {
                graphics.FillPath(trackBrush, track);
            }

            int fillWidth = (int)Math.Round(barBounds.Width * (remaining / 100.0));
            if (fillWidth < 0)
            {
                fillWidth = 0;
            }
            if (fillWidth > barBounds.Width)
            {
                fillWidth = barBounds.Width;
            }

            Color fillColor = GetBarColor(remaining, dark);
            if (fillWidth > 0)
            {
                int radius = Math.Min(ScaleMetric(5), Math.Min(fillWidth, barBounds.Height) / 2);
                using (GraphicsPath fill = RoundedRectangle(
                    new Rectangle(barBounds.X, barBounds.Y, fillWidth, barBounds.Height), radius))
                using (Brush fillBrush = new SolidBrush(fillColor))
                {
                    graphics.FillPath(fillBrush, fill);
                }
            }

            int percentWidth = ScaleMetric(36);
            bool percentInside = fillWidth >= percentWidth + ScaleMetric(4);
            int percentX = percentInside ? barBounds.X + fillWidth - percentWidth : barBounds.X + fillWidth;
            percentX = Math.Min(percentX, barBounds.Right - percentWidth);
            Rectangle percentBounds = new Rectangle(percentX, barBounds.Y, percentWidth, barBounds.Height);
            Color percentBackground = percentInside ? fillColor : trackColor;
            Color percentColor = GetContrastingTextColor(percentBackground);
            if (SystemInformation.HighContrast)
            {
                percentColor = percentInside
                    ? SystemColors.HighlightText
                    : SystemColors.WindowText;
            }
            TextRenderer.DrawText(
                graphics,
                remaining.ToString(CultureInfo.CurrentCulture) + "%",
                percentFont,
                percentBounds,
                percentColor,
                singleLine | TextFormatFlags.HorizontalCenter);
        }

        private void DrawUsageIcon(Graphics graphics, Rectangle bounds, bool weekly, bool dark)
        {
            Color accent = SystemInformation.HighContrast ? SystemColors.Highlight :
                weekly ? (dark ? Color.FromArgb(100, 187, 255) : Color.FromArgb(0, 105, 185)) :
                    (dark ? Color.FromArgb(255, 205, 0) : Color.FromArgb(145, 96, 0));
            Color background = SystemInformation.HighContrast ? SystemColors.Window :
                weekly ? (dark ? Color.FromArgb(39, 65, 88) : Color.FromArgb(218, 235, 250)) :
                    (dark ? Color.FromArgb(82, 70, 29) : Color.FromArgb(255, 246, 204));
            using (GraphicsPath tile = RoundedRectangle(bounds, ScaleMetric(6)))
            using (Brush brush = new SolidBrush(background)) graphics.FillPath(brush, tile);
            float s = (currentDpi / 96.0f) * textScale;
            float left = bounds.X + 9 * s;
            float top = bounds.Y + 8 * s;
            using (Pen pen = new Pen(accent, Math.Max(1.5f, 1.6f * s)))
            {
                pen.LineJoin = LineJoin.Round;
                if (weekly)
                {
                    graphics.DrawRectangle(pen, left, top + 3 * s, 16 * s, 15 * s);
                    graphics.DrawLine(pen, left, top + 7 * s, left + 16 * s, top + 7 * s);
                    graphics.DrawLine(pen, left + 4 * s, top, left + 4 * s, top + 5 * s);
                    graphics.DrawLine(pen, left + 12 * s, top, left + 12 * s, top + 5 * s);
                }
                else
                {
                    graphics.DrawLines(pen, new PointF[] {
                        new PointF(left + 10 * s, top), new PointF(left + 2 * s, top + 10 * s),
                        new PointF(left + 8 * s, top + 10 * s), new PointF(left + 5 * s, top + 19 * s),
                        new PointF(left + 16 * s, top + 7 * s), new PointF(left + 10 * s, top + 7 * s),
                        new PointF(left + 10 * s, top) });
                }
            }
        }

        private void DrawMeasuredDetails(
            Graphics graphics,
            string leftText,
            string rightText,
            Rectangle bounds,
            Color color)
        {
            TextFormatFlags measureFlags = TextFormatFlags.SingleLine |
                TextFormatFlags.NoPrefix |
                TextFormatFlags.NoPadding;
            TextFormatFlags drawFlags = measureFlags |
                TextFormatFlags.VerticalCenter |
                TextFormatFlags.EndEllipsis | TextFormatFlags.PreserveGraphicsClipping;

            if (string.IsNullOrEmpty(leftText))
            {
                if (!string.IsNullOrEmpty(rightText))
                {
                    TextRenderer.DrawText(
                        graphics,
                        rightText,
                        resetDetailFont,
                        bounds,
                        color,
                        drawFlags | TextFormatFlags.Right);
                }
                return;
            }

            if (string.IsNullOrEmpty(rightText))
            {
                TextRenderer.DrawText(graphics, leftText, resetDetailFont, bounds, color, drawFlags);
                return;
            }

            Size leftSize = TextRenderer.MeasureText(
                graphics,
                leftText,
                resetDetailFont,
                new Size(32767, bounds.Height),
                measureFlags);
            Size rightSize = TextRenderer.MeasureText(
                graphics,
                rightText,
                resetDetailFont,
                new Size(32767, bounds.Height),
                measureFlags);
            int gap = ScaleMetric(12);

            if (leftSize.Width + gap + rightSize.Width <= bounds.Width)
            {
                Rectangle leftBounds = new Rectangle(bounds.X, bounds.Y, leftSize.Width, bounds.Height);
                Rectangle rightBounds = new Rectangle(
                    bounds.Right - rightSize.Width,
                    bounds.Y,
                    rightSize.Width,
                    bounds.Height);
                TextRenderer.DrawText(graphics, leftText, resetDetailFont, leftBounds, color, drawFlags);
                TextRenderer.DrawText(
                    graphics,
                    rightText,
                    resetDetailFont,
                    rightBounds,
                    color,
                    drawFlags | TextFormatFlags.Right);
                return;
            }

            int availableWidth = Math.Max(0, bounds.Width - gap);
            int leftWidth = Math.Min(leftSize.Width, availableWidth / 3);
            int minimumLeftWidth = Math.Min(ScaleMetric(76), availableWidth);
            if (leftWidth < minimumLeftWidth)
            {
                leftWidth = minimumLeftWidth;
            }
            int rightWidth = Math.Max(0, availableWidth - leftWidth);

            Rectangle constrainedLeft = new Rectangle(bounds.X, bounds.Y, leftWidth, bounds.Height);
            Rectangle constrainedRight = new Rectangle(
                constrainedLeft.Right + gap,
                bounds.Y,
                rightWidth,
                bounds.Height);
            TextRenderer.DrawText(graphics, leftText, resetDetailFont, constrainedLeft, color, drawFlags);
            TextRenderer.DrawText(
                graphics,
                rightText,
                resetDetailFont,
                constrainedRight,
                color,
                drawFlags | TextFormatFlags.Right);
        }

        private void DrawFooter(
            Graphics graphics,
            Color textColor,
            Color mutedColor,
            Color borderColor,
            bool dark,
            int offset)
        {
            int footerContentTop = GetLogicalFooterContentTop();
            int dividerY = ScaleMetric(footerContentTop - 6) - offset;
            if (GetVisibleUsageRowCount() > 0)
            {
                using (Pen dividerPen = new Pen(borderColor, Math.Max(1.0f, currentDpi / 96.0f)))
                {
                    graphics.DrawLine(
                        dividerPen,
                        ScaleMetric(10),
                        dividerY,
                        BodyWidth - ScaleMetric(10),
                        dividerY);
                }
            }

            int y = ScaleMetric(footerContentTop) - offset;
            string statusLine = BuildStatusLine();
            if (!string.IsNullOrEmpty(statusLine))
            {
                int statusLines = GetStatusLineCount();
                Rectangle statusBounds = new Rectangle(
                    ScaleMetric(14),
                    y,
                    BodyWidth - ScaleMetric(28),
                    ScaleMetric(statusLines * LogicalFooterLineHeight));
                TextRenderer.DrawText(
                    graphics,
                    statusLine,
                    detailFont,
                    statusBounds,
                    GetStatusColor(dark),
                    TextFormatFlags.WordBreak | TextFormatFlags.TextBoxControl |
                        TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding |
                        TextFormatFlags.PreserveGraphicsClipping);
                y += ScaleMetric(statusLines * LogicalFooterLineHeight);
            }

            if (HasResetDetails())
            {
                DrawResetDetails(graphics, y, textColor, mutedColor);
            }
        }

        private void DrawResetDetails(
            Graphics graphics,
            int y,
            Color textColor,
            Color mutedColor)
        {
            if (HasResetDetails())
            {
                int knownCount = snapshot.AvailableResets != null
                    ? snapshot.AvailableResets.Count
                    : 0;
                int displayCount = GetResetDisplayCount();
                string heading = "Limit resets (" +
                    displayCount.ToString(CultureInfo.CurrentCulture) +
                    " available)";
                DrawDetailHeading(graphics, heading, y, textColor);
                y += ScaleMetric(LogicalFooterLineHeight);

                int detailLines = GetResetDetailLineCount() - 1;
                Rectangle panel = new Rectangle(ScaleMetric(14), y, BodyWidth - ScaleMetric(28),
                    ScaleMetric(detailLines * LogicalFooterLineHeight +
                        GetResetEntryCount() * LogicalResetEntryPadding * 2));
                using (GraphicsPath path = RoundedRectangle(panel, ScaleMetric(5)))
                using (Brush brush = new SolidBrush(GetCardColor(IsDarkTheme())))
                using (Pen pen = new Pen(GetBorderColor(IsDarkTheme()), Math.Max(1.0f, currentDpi / 96.0f)))
                {
                    graphics.FillPath(brush, path);
                    graphics.DrawPath(pen, path);
                }

                int entriesTop = y;
                int logicalOffset = 0;
                for (int index = 0; index < knownCount; index++)
                {
                    RateLimitResetCredit credit = snapshot.AvailableResets[index];
                    string title = GetResetTitle(credit, index);
                    string expiration = GetResetExpiration(credit);
                    int entryLines = DrawResetEntry(graphics, title, expiration,
                        entriesTop + ScaleMetric(logicalOffset + LogicalResetEntryPadding), mutedColor);
                    logicalOffset += entryLines * LogicalFooterLineHeight + LogicalResetEntryPadding * 2;
                    int separatorY = entriesTop + ScaleMetric(logicalOffset);
                    if (separatorY < panel.Bottom)
                        using (Pen pen = new Pen(GetBorderColor(IsDarkTheme()), Math.Max(1.0f, currentDpi / 96.0f)))
                            graphics.DrawLine(pen, panel.Left, separatorY, panel.Right, separatorY);
                }

                int unitemizedCount = Math.Max(0, displayCount - knownCount);
                if (knownCount == 0 || unitemizedCount > 0)
                {
                    string label = GetUnitemizedLabel(displayCount, unitemizedCount);
                    string detail = displayCount == 0 ? "" : "Expiration unavailable";
                    DrawResetEntry(graphics, label, detail,
                        entriesTop + ScaleMetric(logicalOffset + LogicalResetEntryPadding), mutedColor);
                }
            }
        }

        private void DrawDetailHeading(Graphics graphics, string text, int y, Color color)
        {
            TextRenderer.DrawText(
                graphics,
                string.IsNullOrEmpty(text) ? "Limit details" : text,
                detailHeadingFont,
                new Rectangle(
                    ScaleMetric(14),
                    y,
                    BodyWidth - ScaleMetric(28),
                    ScaleMetric(17)),
                color,
                TextFormatFlags.SingleLine |
                    TextFormatFlags.VerticalCenter |
                    TextFormatFlags.EndEllipsis |
                    TextFormatFlags.NoPrefix |
                TextFormatFlags.NoPadding | TextFormatFlags.PreserveGraphicsClipping);
        }

        private Rectangle CreateDetailBounds(int y)
        {
            int left = ScaleMetric(26);
            return new Rectangle(
                left,
                y,
                Math.Max(1, BodyWidth - left - ScaleMetric(26)),
                ScaleMetric(17));
        }

        private bool HasResetDetails()
        {
            return GetResetDisplayCount() > 0;
        }

        private int GetResetDisplayCount()
        {
            if (snapshot == null)
            {
                return 0;
            }

            int knownCount = snapshot.AvailableResets != null
                ? snapshot.AvailableResets.Count
                : 0;
            int reportedCount = snapshot.AvailableResetCount.HasValue
                ? snapshot.AvailableResetCount.Value
                : 0;
            return Math.Max(knownCount, Math.Max(0, reportedCount));
        }

        private int GetResetDetailLineCount()
        {
            int lineCount = 0;
            if (HasResetDetails())
            {
                int knownCount = snapshot.AvailableResets != null
                    ? snapshot.AvailableResets.Count
                    : 0;
                int displayCount = GetResetDisplayCount();
                lineCount++;
                for (int index = 0; index < knownCount; index++)
                {
                    RateLimitResetCredit credit = snapshot.AvailableResets[index];
                    lineCount += GetResetEntryLineCount(GetResetTitle(credit, index), GetResetExpiration(credit));
                }
                if (knownCount == 0 || displayCount > knownCount)
                {
                    int unitemized = Math.Max(0, displayCount - knownCount);
                    lineCount += GetResetEntryLineCount(GetUnitemizedLabel(displayCount, unitemized),
                        displayCount == 0 ? "" : "Expiration unavailable");
                }
            }

            return lineCount;
        }

        private int GetResetEntryCount()
        {
            if (!HasResetDetails()) return 0;
            int knownCount = snapshot.AvailableResets != null ? snapshot.AvailableResets.Count : 0;
            return knownCount + (knownCount == 0 || GetResetDisplayCount() > knownCount ? 1 : 0);
        }

        private static string GetResetTitle(RateLimitResetCredit credit, int index)
        {
            string title = credit == null ? null : CompactSingleLine(credit.Title, 1024);
            return (index + 1).ToString(CultureInfo.CurrentCulture) + ". " +
                (string.IsNullOrEmpty(title) ? "Reset" : title);
        }

        private static string GetUnitemizedLabel(int displayCount, int unitemizedCount)
        {
            return displayCount == 0 ? "No resets available" : unitemizedCount.ToString(CultureInfo.CurrentCulture) +
                (unitemizedCount == 1 ? " additional reset" : " additional resets");
        }

        private int DrawResetEntry(Graphics graphics, string title, string expiration, int y, Color color)
        {
            int entryLines = GetResetEntryLineCount(title, expiration);
            if (entryLines == 1)
            {
                DrawMeasuredDetails(graphics, title, expiration, CreateDetailBounds(y), color);
                return entryLines;
            }
            int titleWidth = Math.Max(1, layoutWidth - ScaleMetric(76));
            int titleLines = GetResetWrappedLineCount(title, titleWidth);
            Rectangle bounds = CreateDetailBounds(y);
            bounds.Height = ScaleMetric(titleLines * LogicalFooterLineHeight);
            TextFormatFlags flags = TextFormatFlags.WordBreak | TextFormatFlags.TextBoxControl |
                TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding | TextFormatFlags.PreserveGraphicsClipping;
            TextRenderer.DrawText(graphics, title, resetDetailFont, bounds, color, flags);
            bounds.Y = bounds.Bottom;
            bounds.Height = ScaleMetric((entryLines - titleLines) * LogicalFooterLineHeight);
            if (!string.IsNullOrEmpty(expiration)) TextRenderer.DrawText(graphics, expiration, resetDetailFont, bounds, color, flags);
            return entryLines;
        }

        private static string GetResetExpiration(RateLimitResetCredit credit)
        {
            return credit != null && credit.ExpiresAtUtc.HasValue
                ? "Expires " + TimeFormatter.FormatDateTime(credit.ExpiresAtUtc.Value.ToLocalTime())
                : "Expiration unavailable";
        }

        private int GetResetEntryLineCount(string title, string expiration)
        {
            int width = Math.Max(1, layoutWidth - ScaleMetric(76));
            TextFormatFlags flags = TextFormatFlags.SingleLine | TextFormatFlags.NoPadding | TextFormatFlags.NoPrefix;
            int titleWidth = TextRenderer.MeasureText(title, resetDetailFont, new Size(32767, 32767), flags).Width;
            int expirationWidth = TextRenderer.MeasureText(expiration, resetDetailFont, new Size(32767, 32767), flags).Width;
            if (titleWidth + ScaleMetric(12) + expirationWidth <= width) return 1;
            return GetResetWrappedLineCount(title, width) +
                (string.IsNullOrEmpty(expiration) ? 0 : GetResetWrappedLineCount(expiration, width));
        }

        private int GetStatusLineCount()
        {
            string status = BuildStatusLine();
            return string.IsNullOrEmpty(status) ? 0 : GetWrappedLineCount(status, Math.Max(1, layoutWidth - ScaleMetric(52)));
        }

        private int GetWrappedLineCount(string text, int width)
        {
            return MeasureWrappedLineCount(text, width, detailFont);
        }

        private int GetResetWrappedLineCount(string text, int width)
        {
            return MeasureWrappedLineCount(text, width, resetDetailFont);
        }

        private static int MeasureWrappedLineCount(string text, int width, Font font)
        {
            TextFormatFlags flags = TextFormatFlags.WordBreak | TextFormatFlags.TextBoxControl |
                TextFormatFlags.NoPrefix | TextFormatFlags.NoPadding;
            int height = TextRenderer.MeasureText(text, font, new Size(Math.Max(1, width), 32767), flags).Height;
            int lineHeight = Math.Max(1, TextRenderer.MeasureText("Ag", font, new Size(32767, 32767),
                TextFormatFlags.SingleLine | TextFormatFlags.NoPadding).Height);
            return Math.Max(1, (height + lineHeight - 1) / lineHeight);
        }

        private string BuildAccessibleDetails()
        {
            if (snapshot == null) return BuildStatusLine();
            List<string> details = new List<string>();
            if (snapshot.Weekly != null)
                details.Add("Weekly: " + snapshot.Weekly.RemainingPercent.ToString(CultureInfo.CurrentCulture) + "% remaining");
            if (snapshot.FiveHour != null)
                details.Add("5-hour: " + snapshot.FiveHour.RemainingPercent.ToString(CultureInfo.CurrentCulture) + "% remaining");
            if (historySummary != null)
            {
                for (int day = 0; day < historySummary.DailyUsage.Length; day++)
                    details.Add(FormatDailyUsageDetail(historySummary.DailyDates[day], historySummary.DailyUsage[day], historySummary.DailyPartial[day]));
            }
            string status = !string.IsNullOrWhiteSpace(snapshot.StatusMessage) ? snapshot.StatusMessage :
                (!string.IsNullOrWhiteSpace(snapshot.ErrorMessage) ? snapshot.ErrorMessage : BuildStatusLine());
            if (!string.IsNullOrWhiteSpace(status)) details.Add(status);
            if (snapshot.AvailableResets != null)
            {
                for (int index = 0; index < snapshot.AvailableResets.Count; index++)
                {
                    RateLimitResetCredit credit = snapshot.AvailableResets[index];
                    string title = credit == null || string.IsNullOrWhiteSpace(credit.Title) ? "Reset" : credit.Title;
                    details.Add((index + 1).ToString(CultureInfo.CurrentCulture) + ". " + title + " — " + GetResetExpiration(credit));
                }
            }
            return details.Count == 0 ? null : string.Join(Environment.NewLine, details.ToArray());
        }

        private string BuildStatusLine()
        {
            if (snapshot == null)
            {
                return "Checking limits...";
            }

            string providedStatus = CompactSingleLine(snapshot.StatusMessage, 1024);
            bool hasUsage = snapshot.HasPrimaryLimit;

            if (IsRefreshActive())
            {
                return hasUsage ? null : "Checking limits...";
            }

            if (snapshot.IsPaused)
            {
                return !string.IsNullOrEmpty(providedStatus)
                    ? providedStatus
                    : BuildShowingStatus("Checks paused", snapshot.LastUpdated);
            }

            if (!string.IsNullOrEmpty(snapshot.ErrorMessage))
            {
                if (!string.IsNullOrEmpty(providedStatus))
                {
                    return providedStatus;
                }
                return hasUsage
                    ? BuildShowingStatus("Refresh failed", snapshot.LastUpdated)
                    : CompactSingleLine(snapshot.ErrorMessage, 1024);
            }

            if (snapshot.IsStale)
            {
                return !string.IsNullOrEmpty(providedStatus)
                    ? providedStatus
                    : BuildShowingStatus("Usage may be stale", snapshot.LastUpdated);
            }

            return providedStatus;
        }

        private static string BuildShowingStatus(string prefix, DateTime lastUpdated)
        {
            if (lastUpdated == DateTime.MinValue)
            {
                return prefix;
            }

            return prefix + " - showing " + TimeFormatter.FormatClock(lastUpdated) + " data";
        }

        private void UpdateLayoutMetrics(bool preserveBottom)
        {
            Rectangle area = Screen.FromPoint(Location).WorkingArea;
            ApplyLayoutMetricsInArea(preserveBottom, area.Height - ScaleMetric(16), area.Width - ScaleMetric(16));
        }

        private void ApplyLayoutMetrics(bool preserveBottom, int maximumHeight)
        {
            Rectangle area = Screen.FromPoint(Location).WorkingArea;
            ApplyLayoutMetricsInArea(preserveBottom, maximumHeight, area.Width - ScaleMetric(16));
        }

#if WPF_UI
        private void InitializeWpfPopup()
        {
            wpfView = new UsagePopupView();
            wpfView.RefreshRequested += delegate { RefreshButton_Click(this, EventArgs.Empty); };
            wpfView.SettingsRequested += delegate { SettingsButton_Click(this, EventArgs.Empty); };
            wpfView.HistoryRequested += delegate
            {
                EventHandler handler = HistoryRequested;
                if (handler != null) handler(this, EventArgs.Empty);
            };
            wpfView.ProviderRequested += delegate
            {
                if (claudeSnapshot == null) return;
                showingClaude = !showingClaude;
                UpdateSelectedProvider();
            };
            wpfHost = new System.Windows.Forms.Integration.ElementHost();
            wpfHost.Dock = DockStyle.Fill;
            wpfHost.Child = wpfView;
            Controls.Add(wpfHost);
            wpfHost.BringToFront();
        }

        private void ApplyWpfLayout(bool preserveBottom, int maximumHeight, int maximumWidth)
        {
            if (updatingLayout) return;
            updatingLayout = true;
            try
            {
                int oldBottom = Bottom;
                layoutWidth = Math.Max(1, Math.Min(ScaleMetric(LogicalWidth), maximumWidth));
                UsagePopupView.State state = new UsagePopupView.State();
                state.Provider = showingClaude ? "Claude" : "Codex";
                state.CanSwitchProvider = claudeSnapshot != null;
                state.Refreshing = IsRefreshActive();
                state.Updated = GetHeaderDetail();
                state.Status = BuildStatusLine();
                state.Dark = IsDarkTheme();
                state.Compact = layoutWidth < ScaleMetric(430);
                if (HasWeeklyHistorySummary() && historySummary != null)
                {
                    state.Daily = historySummary.DailyUsage;
                    state.DailyDates = historySummary.DailyDates;
                    state.DailyPartial = historySummary.DailyPartial;
                }
                if (snapshot != null && snapshot.Weekly != null) AddWpfRow(state, snapshot.Weekly, true);
                if (snapshot != null && snapshot.FiveHour != null) AddWpfRow(state, snapshot.FiveHour, false);
                state.ResetCount = GetResetDisplayCount();
                int knownCount = snapshot != null && snapshot.AvailableResets != null ? snapshot.AvailableResets.Count : 0;
                for (int i = 0; i < knownCount; i++)
                {
                    RateLimitResetCredit credit = snapshot.AvailableResets[i];
                    state.Resets.Add(new UsagePopupView.Reset { Title = GetResetTitle(credit, i), Expiration = GetResetExpiration(credit) });
                }
                int unitemized = Math.Max(0, state.ResetCount - knownCount);
                if (state.ResetCount > 0 && (knownCount == 0 || unitemized > 0))
                    state.Resets.Add(new UsagePopupView.Reset { Title = GetUnitemizedLabel(state.ResetCount, unitemized), Expiration = "Expiration unavailable" });
                wpfView.Update(state);
                double scale = Math.Max(1, currentDpi) / 96.0;
                int naturalHeight = (int)Math.Ceiling(wpfView.MeasureNaturalHeight(layoutWidth / scale) * scale);
                int desiredHeight = Math.Min(naturalHeight, Math.Max(ScaleMetric(80), maximumHeight));
                ClientSize = new Size(layoutWidth, Math.Max(1, desiredHeight));
                bodyScrollable = false;
                bodyScrollBar.Visible = false;
                refreshButton.Visible = false;
                settingsButton.Visible = false;
                historyButton.Visible = false;
                providerButton.Visible = false;
                wpfHost.BringToFront();
                UpdateRegion();
                if (preserveBottom && Visible)
                {
                    Top = oldBottom - Height;
                    ClampToWorkingArea();
                }
            }
            finally { updatingLayout = false; }
        }

        private void AddWpfRow(UsagePopupView.State state, LimitWindow window, bool weekly)
        {
            Color color = GetBarColor(window.RemainingPercent, state.Dark);
            state.Rows.Add(new UsagePopupView.Row
            {
                Title = weekly ? "Weekly" : "5-hour",
                Weekly = weekly,
                Remaining = window.RemainingPercent,
                Reset = GetResetDetail(window, snapshot.LastUpdated),
                Forecast = history != null ? history.GetForecast(weekly, snapshot, DateTime.UtcNow) : null,
                Fill = System.Windows.Media.Color.FromArgb(color.A, color.R, color.G, color.B)
            });
        }
#endif

        private void ApplyLayoutMetricsInArea(bool preserveBottom, int maximumHeight, int maximumWidth)
        {
#if WPF_UI
            if (wpfView != null)
            {
                ApplyWpfLayout(preserveBottom, maximumHeight, maximumWidth);
                return;
            }
#endif
            if (updatingLayout)
            {
                return;
            }

            updatingLayout = true;
            try
            {
                int oldBottom = Bottom;
                layoutWidth = Math.Max(1, Math.Min(ScaleMetric(LogicalWidth), maximumWidth));
                bool hasStatus = !string.IsNullOrEmpty(BuildStatusLine());
                bool hasDetails = HasResetDetails();

                int footerContentTop = GetLogicalFooterContentTop();
                int logicalHeight = footerContentTop + LogicalContentBottomPadding;
                if (hasStatus || hasDetails)
                {
                    logicalHeight = footerContentTop;
                    if (hasStatus)
                    {
                        logicalHeight += GetStatusLineCount() * LogicalFooterLineHeight;
                    }
                    if (hasDetails)
                    {
                        logicalHeight += GetResetDetailLineCount() * LogicalFooterLineHeight +
                            GetResetEntryCount() * LogicalResetEntryPadding * 2;
                    }
                    logicalHeight += LogicalFooterBottomPadding;
                }
                int naturalHeight = ScaleMetric(logicalHeight);
                int bodyTop = ScaleMetric(GetLogicalHeaderBottom());
                int desiredHeight = Math.Min(naturalHeight, Math.Max(bodyTop + ScaleMetric(24), maximumHeight));
                bodyScrollable = naturalHeight > desiredHeight;
                Size desiredSize = new Size(layoutWidth, desiredHeight);
                if (ClientSize != desiredSize)
                {
                    ClientSize = desiredSize;
                }

                int previousScroll = bodyScrollBar.Value;
                bodyScrollBar.Value = 0;
                if (bodyScrollable)
                {
                    int viewportHeight = Math.Max(1, desiredHeight - bodyTop - ScaleMetric(4));
                    bodyScrollBar.Minimum = 0;
                    bodyScrollBar.Maximum = Math.Max(0, naturalHeight - bodyTop - 1);
                    bodyScrollBar.LargeChange = viewportHeight;
                    bodyScrollBar.SmallChange = Math.Max(1, ScaleMetric(LogicalFooterLineHeight));
                    bodyScrollBar.Bounds = new Rectangle(ClientSize.Width - ScaleMetric(20), bodyTop,
                        ScaleMetric(16), viewportHeight);
                    bodyScrollBar.Value = Math.Min(previousScroll, GetMaximumScrollValue());
                }
                bodyScrollBar.Visible = bodyScrollable;

                LayoutActionButtons();
                UpdateRegion();
                Invalidate(true);

                if (preserveBottom && Visible)
                {
                    Top = oldBottom - Height;
                    ClampToWorkingArea();
                }
            }
            finally
            {
                updatingLayout = false;
            }
        }

        private void LayoutActionButtons()
        {
            int size = ScaleMetric(36);
            int top = ScaleMetric(UseNarrowLayout ? 32 + GetHeaderDetailLineCount() * 21 + 7 : 10);
            int right = ScaleMetric(12);
            int gap = ScaleMetric(7);
            settingsButton.Bounds = new Rectangle(ClientSize.Width - right - size, top, size, size);
            refreshButton.Bounds = new Rectangle(settingsButton.Left - gap - size, top, size, size);
            historyButton.Bounds = new Rectangle(refreshButton.Left - gap - size, top, size, size);
            providerButton.Bounds = new Rectangle(ScaleMetric(12), top + (SeparateProviderRow ? ScaleMetric(43) : 0),
                ScaleMetric(104), size);
            foreach (Button button in new[] { settingsButton, refreshButton, historyButton, providerButton })
            {
                Region previous = button.Region;
                using (GraphicsPath path = RoundedRectangle(
                    new Rectangle(Point.Empty, button.Size), ScaleMetric(5)))
                    button.Region = new Region(path);
                if (previous != null) previous.Dispose();
            }
        }

        private int GetLogicalFooterContentTop()
        {
            return GetLogicalHeaderBottom() + (HasWeeklyHistorySummary() ? GetLogicalHistoryHeight() : 0) +
                (snapshot != null && snapshot.Weekly != null ? GetUsageRowHeight(snapshot.Weekly) : 0) +
                (snapshot != null && snapshot.FiveHour != null ? GetUsageRowHeight(snapshot.FiveHour) : 0) +
                LogicalFooterTopGap;
        }

        private int GetVisibleUsageRowCount()
        {
            if (snapshot == null)
            {
                return 0;
            }

            int count = 0;
            if (snapshot.Weekly != null)
            {
                count++;
            }
            if (snapshot.FiveHour != null)
            {
                count++;
            }
            return count;
        }

        private void ClampToWorkingArea()
        {
            Rectangle area = Screen.FromControl(this).WorkingArea;
            int margin = ScaleMetric(8);
            int x = Left;
            int y = Top;

            if (x < area.Left + margin)
            {
                x = area.Left + margin;
            }
            if (x + Width > area.Right - margin)
            {
                x = area.Right - margin - Width;
            }
            if (y < area.Top + margin)
            {
                y = area.Top + margin;
            }
            if (y + Height > area.Bottom - margin)
            {
                y = area.Bottom - margin - Height;
            }

            Location = new Point(x, y);
        }

        private void UpdateActionButtonState()
        {
            bool active = IsRefreshActive();
            if (active)
            {
                refreshButton.Text = "";
                if (!refreshAnimationTimer.Enabled)
                {
                    refreshAnimationAngle = -90;
                    refreshAnimationTimer.Start();
                }
            }
            else
            {
                refreshAnimationTimer.Stop();
                refreshAnimationAngle = -90;
                refreshButton.Text = RefreshGlyph;
            }

            refreshButton.Enabled = !active;
            refreshButton.Cursor = active ? Cursors.Default : Cursors.Hand;
            refreshButton.AccessibleName = active ? "Refreshing usage" :
                showingClaude ? "Refresh Claude account usage" : "Refresh usage";
            refreshButton.AccessibleDescription = refreshButton.AccessibleName;
            actionToolTip.SetToolTip(refreshButton, refreshButton.AccessibleName);
            refreshButton.Invalidate();
        }

        private bool IsRefreshActive()
        {
            return (showingClaude ? claudeRefreshing : refreshing) || (snapshot != null && snapshot.IsRefreshing);
        }

        private void ApplyThemeColors()
        {
            bool dark = IsDarkTheme();
            Color backColor = GetBackColor(dark);
            Color textColor = GetTextColor(dark);
            Color hoverColor = SystemInformation.HighContrast
                ? SystemColors.Highlight
                : (dark ? Color.FromArgb(78, 78, 78) : Color.FromArgb(225, 225, 225));
            Color pressedColor = SystemInformation.HighContrast
                ? SystemColors.HotTrack
                : (dark ? Color.FromArgb(92, 92, 92) : Color.FromArgb(210, 210, 210));

            BackColor = backColor;
            ForeColor = textColor;
            Color buttonColor = GetCardColor(dark);
            ApplyButtonColors(refreshButton, buttonColor, textColor, hoverColor, pressedColor);
            ApplyButtonColors(settingsButton, buttonColor, textColor, hoverColor, pressedColor);
            ApplyButtonColors(historyButton, buttonColor, textColor, hoverColor, pressedColor);
            ApplyButtonColors(providerButton, buttonColor, textColor, hoverColor, pressedColor);
        }

        private static void ApplyButtonColors(
            Button button,
            Color backColor,
            Color textColor,
            Color hoverColor,
            Color pressedColor)
        {
            button.BackColor = backColor;
            button.ForeColor = textColor;
            button.FlatAppearance.BorderColor = GetBorderColor(!SystemInformation.HighContrast &&
                backColor.GetBrightness() < 0.5f);
            button.FlatAppearance.MouseOverBackColor = hoverColor;
            button.FlatAppearance.MouseDownBackColor = pressedColor;
        }

        private bool IsDarkTheme()
        {
            return !SystemInformation.HighContrast &&
                (settings == null || AppSettings.IsDarkTheme(settings.Theme));
        }

        private Color GetBarColor(int remaining, bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.Highlight;
            }
            if (settings == null || settings.ColorBars)
            {
                return IconRenderer.ColorForPercent(remaining, settings);
            }
            return dark ? Color.FromArgb(132, 132, 132) : Color.FromArgb(118, 118, 118);
        }

        private static Color GetBackColor(bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.Window;
            }
            return dark ? Color.FromArgb(19, 24, 31) : Color.FromArgb(248, 249, 251);
        }

        private static Color GetCardColor(bool dark)
        {
            if (SystemInformation.HighContrast) return SystemColors.Window;
            return dark ? Color.FromArgb(31, 36, 43) : Color.White;
        }

        private static Color GetTextColor(bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.WindowText;
            }
            return dark ? Color.White : Color.Black;
        }

        private static Color GetMutedColor(bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.GrayText;
            }
            return dark ? Color.Gainsboro : Color.FromArgb(72, 72, 72);
        }

        private static Color GetBorderColor(bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.WindowFrame;
            }
            return dark ? Color.FromArgb(73, 82, 93) : Color.FromArgb(195, 203, 213);
        }

        private static Color GetTrackColor(bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.ControlDark;
            }
            return dark ? Color.FromArgb(52, 57, 64) : Color.FromArgb(221, 226, 232);
        }

        private static Color GetStatusColor(bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.WindowText;
            }
            return dark ? Color.FromArgb(255, 214, 128) : Color.FromArgb(126, 72, 0);
        }

        private static Color GetContrastingTextColor(Color background)
        {
            double luminance = (background.R * 0.299) +
                (background.G * 0.587) +
                (background.B * 0.114);
            return luminance >= 150.0 ? Color.Black : Color.White;
        }

        private int BodyWidth
        {
            get { return ClientSize.Width - (bodyScrollable ? ScaleMetric(24) : 0); }
        }

        private int LogicalLayoutWidth
        {
            get { return (int)Math.Floor(layoutWidth / ((currentDpi / 96.0) * Math.Max(1.0f, textScale))); }
        }

        internal static float GetTextScale(float messageFontSize)
        {
            return Math.Max(1.0f, messageFontSize / 10.0f);
        }

        private int ScaleMetric(int logicalPixels)
        {
            return (int)Math.Round(logicalPixels * (currentDpi / 96.0) * Math.Max(1.0f, textScale));
        }

        private void UpdateRegion()
        {
            if (Width <= 0 || Height <= 0)
            {
                return;
            }

            Region oldRegion = Region;
            using (GraphicsPath path = RoundedRectangle(
                new Rectangle(0, 0, Width, Height),
                ScaleMetric(8)))
            {
                Region = new Region(path);
            }
            if (oldRegion != null)
            {
                oldRegion.Dispose();
            }
        }

        private static GraphicsPath RoundedRectangle(Rectangle bounds, int radius)
        {
            GraphicsPath path = new GraphicsPath();
            if (radius <= 0)
            {
                path.AddRectangle(bounds);
                return path;
            }

            int diameter = radius * 2;
            path.AddArc(bounds.Left, bounds.Top, diameter, diameter, 180, 90);
            path.AddArc(bounds.Right - diameter - 1, bounds.Top, diameter, diameter, 270, 90);
            path.AddArc(bounds.Right - diameter - 1, bounds.Bottom - diameter - 1, diameter, diameter, 0, 90);
            path.AddArc(bounds.Left, bounds.Bottom - diameter - 1, diameter, diameter, 90, 90);
            path.CloseFigure();
            return path;
        }

        private static string CompactSingleLine(string value, int maxLength)
        {
            if (string.IsNullOrWhiteSpace(value))
            {
                return null;
            }

            string compact = value.Replace('\r', ' ').Replace('\n', ' ').Replace('\t', ' ').Trim();
            while (compact.IndexOf("  ", StringComparison.Ordinal) >= 0)
            {
                compact = compact.Replace("  ", " ");
            }
            if (compact.Length > maxLength)
            {
                compact = compact.Substring(0, Math.Max(0, maxLength - 3)).TrimEnd() + "...";
            }
            return compact;
        }

    }
}
