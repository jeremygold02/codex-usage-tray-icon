using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Globalization;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal sealed class UsageHistoryForm : Form
    {
        private readonly UsageHistoryStore history;
        private readonly RadioButton weeklyButton;
        private readonly RadioButton fiveHourButton;
        private readonly HistoryChart chart;
        private readonly Label forecastLabel;
        private readonly Label detailLabel;
        private readonly ToolTip actionToolTip = new ToolTip();
        private readonly Timer displayTimer = new Timer();
        private AppSettings settings;
        private UsageSnapshot snapshot;

        public event EventHandler HistoryCleared;

        public UsageHistoryForm(UsageHistoryStore history, AppSettings settings)
            : this(history, settings, "Codex")
        {
        }

        public UsageHistoryForm(UsageHistoryStore history, AppSettings settings, string provider)
        {
            if (history == null)
            {
                throw new ArgumentNullException("history");
            }

            this.history = history;
            this.settings = settings;
            Text = provider + " Usage History";
            ClientSize = new Size(620, 420);
            MinimumSize = new Size(490, 360);
            StartPosition = FormStartPosition.CenterScreen;
            Font = SystemFonts.MessageBoxFont;
            AutoScaleDimensions = new SizeF(96.0f, 96.0f);
            AutoScaleMode = AutoScaleMode.Dpi;
            try
            {
                Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath);
            }
            catch
            {
            }

            TableLayoutPanel layout = new TableLayoutPanel();
            layout.Dock = DockStyle.Fill;
            layout.Padding = new Padding(16, 12, 16, 12);
            layout.ColumnCount = 1;
            layout.RowCount = 4;
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            layout.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));

            FlowLayoutPanel tabs = new FlowLayoutPanel();
            tabs.AutoSize = true;
            tabs.Dock = DockStyle.Fill;
            tabs.Margin = new Padding(0, 0, 0, 8);
            weeklyButton = CreatePeriodButton("&Weekly", "Weekly limit: all recorded history");
            fiveHourButton = CreatePeriodButton("&5-hour", "Five-hour limit: all recorded history");
            tabs.Controls.Add(weeklyButton);
            tabs.Controls.Add(fiveHourButton);
            Button clearButton = new Button();
            clearButton.Text = "Clear history";
            clearButton.AccessibleName = "Clear saved usage history";
            clearButton.AutoSize = true;
            clearButton.Margin = new Padding(8, 0, 0, 0);
            clearButton.Click += ClearHistory;
            tabs.Controls.Add(clearButton);
            actionToolTip.SetToolTip(clearButton,
                "History is saved for this Windows user. Clear it after switching " + provider + " accounts.");
            weeklyButton.Checked = true;
            weeklyButton.CheckedChanged += PeriodChanged;
            fiveHourButton.CheckedChanged += PeriodChanged;

            chart = new HistoryChart();
            chart.Dock = DockStyle.Fill;
            chart.Margin = new Padding(0);
            chart.SelectionChanged += ChartSelectionChanged;
            chart.TabIndex = 1;
            detailLabel = CreateLabel("");
            detailLabel.AccessibleName = "Selected usage reading";
            detailLabel.MinimumSize = new Size(0, Font.Height * 2 + 8);
            detailLabel.Margin = new Padding(0, 8, 0, 0);
            forecastLabel = CreateLabel("");
            forecastLabel.AccessibleName = "Estimated usage remaining";

            layout.Controls.Add(tabs, 0, 0);
            layout.Controls.Add(chart, 0, 1);
            layout.Controls.Add(detailLabel, 0, 2);
            layout.Controls.Add(forecastLabel, 0, 3);
            Controls.Add(layout);
            ApplySettings(settings);
            UpdateData(null);
            displayTimer.Interval = 60000;
            displayTimer.Tick += delegate
            {
                if (Visible)
                {
                    UpdateData(snapshot);
                }
            };
        }

        public void UpdateData(UsageSnapshot value)
        {
            snapshot = value;
            DateTime nowUtc = DateTime.UtcNow;
            bool weekly = weeklyButton.Checked;
            chart.UpdateData(history.Samples, weekly, nowUtc);
            chart.UpdateProjection(history.GetProjection(weekly, snapshot, nowUtc));
            forecastLabel.Text = history.GetForecast(weekly, snapshot, nowUtc);
        }

        public void ApplySettings(AppSettings value)
        {
            settings = value;
            bool highContrast = SystemInformation.HighContrast;
            bool dark = !highContrast && (settings == null || AppSettings.IsDarkTheme(settings.Theme));
            Color background = highContrast ? SystemColors.Window :
                dark ? Color.FromArgb(54, 54, 54) : Color.FromArgb(245, 245, 245);
            Color foreground = highContrast ? SystemColors.WindowText : dark ? Color.White : Color.Black;
            ApplyControlColors(this, background, foreground);
            chart.ApplyTheme(dark);
            Invalidate(true);
        }

        protected override void OnSystemColorsChanged(EventArgs e)
        {
            base.OnSystemColorsChanged(e);
            if (chart != null)
            {
                ApplySettings(settings);
            }
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                displayTimer.Stop();
                displayTimer.Dispose();
                actionToolTip.Dispose();
            }
            base.Dispose(disposing);
        }

        protected override void OnVisibleChanged(EventArgs e)
        {
            base.OnVisibleChanged(e);
            if (chart != null && !Disposing && !IsDisposed)
            {
                displayTimer.Enabled = Visible;
                if (Visible)
                {
                    UpdateData(snapshot);
                }
            }
        }

        protected override bool ProcessCmdKey(ref Message msg, Keys keyData)
        {
            if (keyData == Keys.Escape)
            {
                Close();
                return true;
            }
            return base.ProcessCmdKey(ref msg, keyData);
        }

        private void PeriodChanged(object sender, EventArgs e)
        {
            RadioButton selected = sender as RadioButton;
            if (chart != null && selected != null && selected.Checked)
            {
                UpdateData(snapshot);
            }
        }

        private void ClearHistory(object sender, EventArgs e)
        {
            if (MessageBox.Show(this, "Clear all saved usage history? New readings will start a fresh history.",
                "Clear usage history", MessageBoxButtons.OKCancel, MessageBoxIcon.Question,
                MessageBoxDefaultButton.Button2) != DialogResult.OK)
            {
                return;
            }

            bool saved = history.Clear();
            UpdateData(snapshot);
            EventHandler handler = HistoryCleared;
            if (handler != null)
            {
                handler(this, EventArgs.Empty);
            }
            if (!saved)
            {
                MessageBox.Show(this, "History was cleared for this session, but the saved file could not be updated. " +
                    "Previous readings may return after restarting the app.",
                    "Usage history", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            }
        }

        private void ChartSelectionChanged(object sender, EventArgs e)
        {
            detailLabel.Text = string.IsNullOrEmpty(chart.SelectedDetail)
                ? chart.LatestDetail
                : chart.SelectedDetail;
        }

        private static RadioButton CreatePeriodButton(string text, string accessibleName)
        {
            RadioButton button = new RadioButton();
            button.Text = text;
            button.AccessibleName = accessibleName;
            button.AutoSize = true;
            button.Margin = new Padding(0, 0, 20, 0);
            return button;
        }

        private static Label CreateLabel(string text)
        {
            Label label = new Label();
            label.Text = text;
            label.AutoSize = true;
            label.Dock = DockStyle.Fill;
            label.Margin = new Padding(0, 4, 0, 0);
            label.UseMnemonic = false;
            return label;
        }

        private static void ApplyControlColors(Control control, Color background, Color foreground)
        {
            control.BackColor = background;
            control.ForeColor = foreground;
            foreach (Control child in control.Controls)
            {
                ApplyControlColors(child, background, foreground);
            }
        }

        private sealed class HistoryChart : Control
        {
            private readonly List<UsageHistorySample> samples = new List<UsageHistorySample>();
            private DateTime startUtc;
            private DateTime endUtc;
            private DateTime observedEndUtc;
            private UsageProjection projection;
            private bool weekly;
            private bool dark;
            private int selectedIndex = -1;
            private RectangleF plot;

            public event EventHandler SelectionChanged;
            public string SelectedDetail { get; private set; }
            public string LatestDetail
            {
                get { return samples.Count == 0 ? "No recorded readings" : "Latest: " + FormatDetail(samples.Count - 1); }
            }

            public HistoryChart()
            {
                SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint |
                    ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw |
                    ControlStyles.Selectable, true);
                TabStop = true;
                AccessibleName = "Remaining usage history chart";
                AccessibleRole = AccessibleRole.Graphic;
            }

            public void UpdateData(List<UsageHistorySample> values, bool showWeekly, DateTime nowUtc)
            {
                DateTime? selectedTimestamp = weekly == showWeekly && selectedIndex >= 0 && selectedIndex < samples.Count
                    ? (DateTime?)samples[selectedIndex].TimestampUtc
                    : null;
                weekly = showWeekly;
                projection = null;
                observedEndUtc = nowUtc;
                endUtc = nowUtc;
                startUtc = nowUtc;
                samples.Clear();
                if (values != null)
                {
                    for (int i = 0; i < values.Count; i++)
                    {
                        UsageHistorySample sample = values[i];
                        if (sample != null && sample.TimestampUtc <= endUtc &&
                            UsageHistoryStore.GetWindow(sample, weekly) != null)
                        {
                            samples.Add(sample);
                        }
                    }
                }
                samples.Sort(delegate(UsageHistorySample left, UsageHistorySample right)
                {
                    return left.TimestampUtc.CompareTo(right.TimestampUtc);
                });
                if (samples.Count > 0) startUtc = samples[0].TimestampUtc;
                if (endUtc <= startUtc) endUtc = startUtc.AddMinutes(1);
                int retainedIndex = -1;
                if (selectedTimestamp.HasValue)
                {
                    for (int i = 0; i < samples.Count; i++)
                    {
                        if (samples[i].TimestampUtc == selectedTimestamp.Value &&
                            UsageHistoryStore.GetWindow(samples[i], weekly) != null)
                        {
                            retainedIndex = i;
                            break;
                        }
                    }
                }
                selectedIndex = -1;
                SelectSample(retainedIndex);
                Invalidate();
            }

            public void UpdateProjection(UsageProjection value)
            {
                projection = null;
                if (value != null && samples.Count > 0)
                {
                    UsageHistorySample latest = samples[samples.Count - 1];
                    UsageHistoryWindow window = UsageHistoryStore.GetWindow(latest, weekly);
                    if (Math.Abs((latest.TimestampUtc - value.StartUtc).TotalSeconds) <= 1 &&
                        Math.Abs(100 - window.UsedPercent - value.StartRemainingPercent) < 0.001 &&
                        value.EndUtc > value.StartUtc)
                        projection = value;
                }
                endUtc = projection != null && projection.EndUtc > observedEndUtc ? projection.EndUtc : observedEndUtc;
                if (endUtc <= startUtc) endUtc = startUtc.AddMinutes(1);
                Invalidate();
            }

            public void ApplyTheme(bool isDark)
            {
                dark = isDark;
                Invalidate();
            }

            protected override void OnPaint(PaintEventArgs e)
            {
                base.OnPaint(e);
                Graphics graphics = e.Graphics;
                graphics.SmoothingMode = SmoothingMode.AntiAlias;
                float scale = graphics.DpiX / 96.0f;
                float labelHeight = Math.Max(Font.Height + 4, 18 * scale);
                bool showDates = startUtc.ToLocalTime().Date != endUtc.ToLocalTime().Date;
                plot = new RectangleF(48 * scale, labelHeight + 4 * scale,
                    Math.Max(1, Width - 64 * scale), Math.Max(1, Height - labelHeight * (showDates ? 4 : 3) - 8 * scale));
                Color muted = SystemInformation.HighContrast ? SystemColors.WindowText :
                    dark ? Color.Gainsboro : Color.FromArgb(72, 72, 72);
                Color grid = SystemInformation.HighContrast ? SystemColors.WindowText :
                    dark ? Color.FromArgb(90, 90, 90) : Color.FromArgb(195, 195, 195);
                Color line = SystemInformation.HighContrast ? SystemColors.Highlight :
                    dark ? Color.FromArgb(100, 195, 255) : Color.FromArgb(0, 104, 180);
                Color projectionColor = SystemInformation.HighContrast ? SystemColors.WindowText :
                    dark ? Color.FromArgb(242, 178, 77) : Color.FromArgb(160, 89, 0);
                DrawText(graphics, "Remaining", new RectangleF(0, 0, Width, labelHeight), muted, false);
                if (projection != null)
                {
                    PointF projectedStart = GetChartPoint(projection.StartUtc, projection.StartRemainingPercent);
                    using (Brush tint = new SolidBrush(Color.FromArgb(18, projectionColor)))
                        graphics.FillRectangle(tint, projectedStart.X, plot.Top, Math.Max(0, plot.Right - projectedStart.X), plot.Height);
                    DrawText(graphics, "Projected", new RectangleF(plot.Right - 100 * scale, 0, 100 * scale, labelHeight), projectionColor, true);
                }

                using (Pen gridPen = new Pen(grid, 1))
                {
                    for (int percent = 0; percent <= 100; percent += 25)
                    {
                        float y = plot.Bottom - plot.Height * percent / 100.0f;
                        graphics.DrawLine(gridPen, plot.Left, y, plot.Right, y);
                        DrawText(graphics, percent.ToString(CultureInfo.CurrentCulture) + "%",
                            new RectangleF(0, y - labelHeight / 2, plot.Left - 6 * scale, labelHeight), muted, true);
                    }
                    for (int tick = 0; samples.Count > 0 && tick <= 4; tick++)
                    {
                        float x = plot.Left + plot.Width * tick / 4.0f;
                        graphics.DrawLine(gridPen, x, plot.Bottom, x, plot.Bottom + 4 * scale);
                        DateTime time = startUtc + TimeSpan.FromTicks((endUtc - startUtc).Ticks * tick / 4);
                        DateTime local = time.ToLocalTime();
                        string label = local.ToString(showDates ? "MMM d" : "t", CultureInfo.CurrentCulture);
                        float width = Math.Min(88 * scale, plot.Width / 4);
                        float left = Math.Max(plot.Left, Math.Min(plot.Right - width, x - width / 2));
                        DrawText(graphics, label, new RectangleF(left, plot.Bottom + 6 * scale, width, labelHeight), muted, false);
                        if (showDates)
                            DrawText(graphics, local.ToString("t", CultureInfo.CurrentCulture),
                                new RectangleF(left, plot.Bottom + 6 * scale + labelHeight, width, labelHeight), muted, false);
                    }
                }

                if (projection != null)
                {
                    PointF projectedStart = GetChartPoint(projection.StartUtc, projection.StartRemainingPercent);
                    PointF projectedEnd = GetChartPoint(projection.EndUtc, projection.EndRemainingPercent);
                    using (Pen projectionPen = new Pen(projectionColor, Math.Max(1.5f, 2 * scale)))
                    {
                        projectionPen.DashStyle = DashStyle.Dash;
                        graphics.DrawLine(projectionPen, projectedStart, projectedEnd);
                        projectionPen.DashStyle = DashStyle.Solid;
                        graphics.DrawEllipse(projectionPen, projectedEnd.X - 3 * scale, projectedEnd.Y - 3 * scale, 6 * scale, 6 * scale);
                    }
                }

                int validCount = 0;
                using (Pen linePen = new Pen(line, Math.Max(1.5f, 2 * scale)))
                using (Pen gapPen = new Pen(line, Math.Max(1.5f, 2 * scale)))
                using (Pen guidePen = new Pen(SystemInformation.HighContrast ? line : Color.FromArgb(160, line), Math.Max(1, scale)))
                using (Pen resetPen = new Pen(muted, Math.Max(1, scale)))
                using (Brush pointBrush = new SolidBrush(line))
                {
                    resetPen.DashStyle = DashStyle.Dot;
                    gapPen.DashStyle = DashStyle.Dash;
                    if (selectedIndex >= 0 && selectedIndex < samples.Count)
                    {
                        PointF selected = GetPoint(samples[selectedIndex], UsageHistoryStore.GetWindow(samples[selectedIndex], weekly));
                        graphics.DrawLine(guidePen, selected.X, plot.Top, selected.X, plot.Bottom);
                    }
                    for (int i = 0; i < samples.Count; i++)
                    {
                        UsageHistoryWindow window = UsageHistoryStore.GetWindow(samples[i], weekly);
                        if (window == null)
                        {
                            continue;
                        }
                        validCount++;
                        PointF point = GetPoint(samples[i], window);
                        if (i > 0 && !UsageHistoryStore.IsReset(samples[i - 1], samples[i], weekly))
                        {
                            Pen segmentPen = UsageHistoryStore.IsContinuous(samples[i - 1], samples[i], weekly) ? linePen : gapPen;
                            graphics.DrawLine(segmentPen,
                                GetPoint(samples[i - 1], UsageHistoryStore.GetWindow(samples[i - 1], weekly)), point);
                        }
                        if (i > 0 && UsageHistoryStore.IsReset(samples[i - 1], samples[i], weekly))
                        {
                            graphics.DrawLine(resetPen, point.X, plot.Top, point.X, plot.Bottom);
                            graphics.FillRectangle(pointBrush, point.X - 3 * scale, point.Y - 3 * scale, 6 * scale, 6 * scale);
                        }
                        else if (i == 0 || i == samples.Count - 1 || i == selectedIndex)
                        {
                            graphics.FillEllipse(pointBrush, point.X - 2 * scale, point.Y - 2 * scale, 4 * scale, 4 * scale);
                        }
                        if (i == selectedIndex)
                        {
                            graphics.DrawEllipse(linePen, point.X - 5 * scale, point.Y - 5 * scale, 10 * scale, 10 * scale);
                        }
                    }
                }

                if (validCount == 0)
                {
                    using (Brush background = new SolidBrush(BackColor))
                    {
                        graphics.FillRectangle(background, plot.Left + 1, plot.Top + 1, plot.Width - 2, plot.Height - 2);
                    }
                    TextRenderer.DrawText(graphics, "No readings recorded yet.\nHistory starts with successful usage refreshes.",
                        Font, Rectangle.Round(plot), ForeColor,
                        TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.WordBreak);
                }
            }

            protected override void OnMouseMove(MouseEventArgs e)
            {
                base.OnMouseMove(e);
                if (!plot.Contains(e.Location))
                {
                    SelectSample(-1);
                    return;
                }
                int nearest = -1;
                double distance = double.MaxValue;
                for (int i = 0; i < samples.Count; i++)
                {
                    UsageHistoryWindow window = UsageHistoryStore.GetWindow(samples[i], weekly);
                    if (window == null)
                    {
                        continue;
                    }
                    double candidate = Math.Abs(GetPoint(samples[i], window).X - e.X);
                    if (candidate < distance)
                    {
                        distance = candidate;
                        nearest = i;
                    }
                }
                SelectSample(nearest);
            }

            protected override void OnMouseLeave(EventArgs e)
            {
                base.OnMouseLeave(e);
                SelectSample(-1);
            }

            protected override void OnMouseDown(MouseEventArgs e)
            {
                base.OnMouseDown(e);
                Focus();
            }

            protected override bool IsInputKey(Keys keyData)
            {
                Keys key = keyData & Keys.KeyCode;
                return key == Keys.Left || key == Keys.Right || key == Keys.Home || key == Keys.End || base.IsInputKey(keyData);
            }

            protected override void OnKeyDown(KeyEventArgs e)
            {
                base.OnKeyDown(e);
                if (e.KeyCode != Keys.Left && e.KeyCode != Keys.Right && e.KeyCode != Keys.Home && e.KeyCode != Keys.End)
                {
                    return;
                }
                int direction = e.KeyCode == Keys.Left || e.KeyCode == Keys.End ? -1 : 1;
                int next = selectedIndex;
                if (e.KeyCode == Keys.Home || e.KeyCode == Keys.End || next < 0)
                {
                    next = direction > 0 ? -1 : samples.Count;
                }
                do
                {
                    next += direction;
                }
                while (next >= 0 && next < samples.Count && UsageHistoryStore.GetWindow(samples[next], weekly) == null);
                if (next >= 0 && next < samples.Count)
                {
                    SelectSample(next);
                }
                e.Handled = true;
                e.SuppressKeyPress = true;
            }

            protected override void OnGotFocus(EventArgs e)
            {
                base.OnGotFocus(e);
                if (selectedIndex < 0 && samples.Count > 0) SelectSample(samples.Count - 1);
            }

            protected override void OnLostFocus(EventArgs e)
            {
                base.OnLostFocus(e);
                SelectSample(-1);
            }

            private string FormatDetail(int index)
            {
                UsageHistorySample sample = samples[index];
                UsageHistoryWindow window = UsageHistoryStore.GetWindow(sample, weekly);
                string detail = sample.TimestampUtc.ToLocalTime().ToString("G", CultureInfo.CurrentCulture) +
                    Environment.NewLine + (100 - window.UsedPercent).ToString("0.#", CultureInfo.CurrentCulture) +
                    "% remaining  |  " + window.UsedPercent.ToString("0.#", CultureInfo.CurrentCulture) + "% used";
                if (index > 0 && UsageHistoryStore.IsReset(samples[index - 1], sample, weekly))
                    detail += "  |  Reset or adjustment";
                return detail;
            }

            private void SelectSample(int index)
            {
                if (index == selectedIndex && index >= 0)
                {
                    return;
                }
                selectedIndex = index;
                SelectedDetail = index < 0 ? "" : FormatDetail(index);
                AccessibleDescription = string.IsNullOrEmpty(SelectedDetail)
                    ? LatestDetail + ". Use Left and Right to inspect recorded readings."
                    : SelectedDetail;
                EventHandler handler = SelectionChanged;
                if (handler != null)
                {
                    handler(this, EventArgs.Empty);
                }
                Invalidate();
            }

            private PointF GetPoint(UsageHistorySample sample, UsageHistoryWindow window)
            {
                return GetChartPoint(sample.TimestampUtc, 100 - window.UsedPercent);
            }

            private PointF GetChartPoint(DateTime timestamp, double remainingPercent)
            {
                double fraction = (timestamp - startUtc).TotalSeconds / (endUtc - startUtc).TotalSeconds;
                double remaining = Math.Max(0, Math.Min(100, remainingPercent));
                return new PointF(plot.Left + (float)(plot.Width * fraction),
                    plot.Bottom - (float)(plot.Height * remaining / 100));
            }

            private void DrawText(Graphics graphics, string text, RectangleF bounds, Color color, bool alignRight)
            {
                TextRenderer.DrawText(graphics, text, Font, Rectangle.Round(bounds), color,
                    TextFormatFlags.NoPadding | TextFormatFlags.NoPrefix | TextFormatFlags.SingleLine |
                    TextFormatFlags.VerticalCenter | (alignRight ? TextFormatFlags.Right : TextFormatFlags.Left));
            }
        }
    }
}
