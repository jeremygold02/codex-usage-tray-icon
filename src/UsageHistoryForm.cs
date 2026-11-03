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
        private readonly Label remainingValue;
        private readonly Label resetValue;
        private readonly Label summaryForecastValue;
        private readonly Font bodyFont;
        private readonly Font remainingFont;
        private readonly Timer displayTimer = new Timer();
#if WPF_UI
        private HistoryHeader historyHeader;
#endif
        private AppSettings settings;
        private UsageSnapshot snapshot;

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
            ClientSize = new Size(760, 500);
            MinimumSize = new Size(660, 420);
#if WPF_UI
            ClientSize = new Size(1000, 700);
            MinimumSize = new Size(760, 540);
#endif
            StartPosition = FormStartPosition.CenterScreen;
            bodyFont = new Font(SystemFonts.MessageBoxFont.FontFamily,
                Math.Max(10f, SystemFonts.MessageBoxFont.SizeInPoints));
            Font = bodyFont;
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
            layout.Padding = new Padding(26, 18, 26, 18);
            layout.ColumnCount = 1;
            layout.RowCount = 4;
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            layout.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));

            FlowLayoutPanel tabs = new FlowLayoutPanel();
            tabs.AutoSize = true;
            tabs.Dock = DockStyle.Fill;
            tabs.Margin = new Padding(0, 0, 0, 16);
            weeklyButton = CreatePeriodButton("&Weekly", "Weekly limit: history since the latest reset");
            fiveHourButton = CreatePeriodButton("&5-hour", "Five-hour limit: history since the latest reset");
            tabs.Controls.Add(weeklyButton);
            tabs.Controls.Add(fiveHourButton);
            weeklyButton.Checked = true;
            weeklyButton.CheckedChanged += PeriodChanged;
            fiveHourButton.CheckedChanged += PeriodChanged;

            TableLayoutPanel summary = new TableLayoutPanel();
            summary.Dock = DockStyle.Fill;
            summary.AutoSize = true;
            summary.Margin = new Padding(0, 0, 0, 16);
            summary.ColumnCount = 3;
            summary.RowCount = 1;
            summary.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 20));
            summary.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 34));
            summary.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 46));
            summary.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            remainingFont = new Font(Font.FontFamily, Math.Max(18f, Font.SizeInPoints * 1.8f), FontStyle.Bold);
            remainingValue = CreateSummaryValue();
            remainingValue.Font = remainingFont;
            resetValue = CreateSummaryValue();
            summaryForecastValue = CreateSummaryValue();
            summary.Controls.Add(CreateSummaryBlock("Remaining", remainingValue, ""), 0, 0);
            summary.Controls.Add(CreateSummaryBlock("Next reset", resetValue, "calendar"), 1, 0);
            summary.Controls.Add(CreateSummaryBlock("Forecast", summaryForecastValue, "forecast"), 2, 0);
            summary.Paint += delegate(object sender, PaintEventArgs e)
            {
                int[] widths = summary.GetColumnWidths();
                using (Pen divider = new Pen(DividerColor()))
                {
                    int x = widths[0] - 8;
                    e.Graphics.DrawLine(divider, x, 2, x, summary.Height - 2);
                    x += widths[1];
                    e.Graphics.DrawLine(divider, x, 2, x, summary.Height - 2);
                }
            };

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

            TableLayoutPanel footer = new TableLayoutPanel();
            footer.Dock = DockStyle.Fill;
            footer.AutoSize = true;
            footer.ColumnCount = 2;
            footer.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 48));
            footer.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 52));
            footer.Controls.Add(detailLabel, 0, 0);
            footer.Controls.Add(forecastLabel, 1, 0);
            forecastLabel.Margin = new Padding(16, 8, 0, 0);
            footer.Paint += delegate(object sender, PaintEventArgs e)
            {
                using (Pen divider = new Pen(DividerColor()))
                    e.Graphics.DrawLine(divider, 0, 0, footer.Width, 0);
            };

#if WPF_UI
            // Keep the original controls as the state source and retain their
            // event wiring; the visible header presents that same live state.
            tabs.Visible = false;
            summary.Visible = false;
            Controls.Add(tabs);
            Controls.Add(summary);
            historyHeader = new HistoryHeader();
            historyHeader.PeriodChanged += delegate
            {
                if (historyHeader.IsWeekly) weeklyButton.Checked = true;
                else if (snapshot != null && snapshot.FiveHour != null) fiveHourButton.Checked = true;
            };
            System.Windows.Forms.Integration.ElementHost headerHost = new System.Windows.Forms.Integration.ElementHost();
            headerHost.Dock = DockStyle.Fill;
            headerHost.Height = 142;
            headerHost.Margin = new Padding(0, 0, 0, 12);
            headerHost.Child = historyHeader;
            layout.Controls.Add(headerHost, 0, 0);
#else
            layout.Controls.Add(tabs, 0, 0);
            layout.Controls.Add(summary, 0, 1);
#endif
            layout.Controls.Add(chart, 0, 2);
            layout.Controls.Add(footer, 0, 3);
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
            bool hasFiveHourLimit = snapshot != null && snapshot.FiveHour != null;
            fiveHourButton.Visible = hasFiveHourLimit;
            if (!hasFiveHourLimit && fiveHourButton.Checked)
            {
                weeklyButton.Checked = true;
                return; // CheckedChanged updates the chart for the weekly limit.
            }
            DateTime nowUtc = DateTime.UtcNow;
            bool weekly = weeklyButton.Checked;
            chart.UpdateData(history.Samples, weekly, nowUtc);
            chart.UpdateProjection(settings == null || settings.ShowProjectedUsage
                ? history.GetProjection(weekly, snapshot, nowUtc) : null);
            forecastLabel.Text = history.GetForecast(weekly, snapshot, nowUtc);
            summaryForecastValue.Text = forecastLabel.Text;
            LimitWindow limit = snapshot == null ? null : (weekly ? snapshot.Weekly : snapshot.FiveHour);
            remainingValue.Text = limit == null ? "\u2014" : limit.RemainingPercent.ToString(CultureInfo.CurrentCulture) + "%";
            resetValue.Text = limit == null || !limit.ResetAfterSeconds.HasValue ? "\u2014" :
                TimeFormatter.FormatResetDateTime(snapshot.LastUpdated.ToLocalTime(), limit.ResetAfterSeconds.Value);
#if WPF_UI
            if (historyHeader != null)
                historyHeader.SetValues(weekly, hasFiveHourLimit, remainingValue.Text, resetValue.Text, summaryForecastValue.Text);
#endif
        }

        public void ApplySettings(AppSettings value)
        {
            settings = value;
            bool highContrast = SystemInformation.HighContrast;
            bool dark = !highContrast && (settings == null || AppSettings.IsDarkTheme(settings.Theme));
            Color background = highContrast ? SystemColors.Window :
                dark ? Color.FromArgb(19, 24, 31) : Color.FromArgb(248, 249, 251);
            Color foreground = highContrast ? SystemColors.WindowText : dark ? Color.White : Color.Black;
            ApplyControlColors(this, background, foreground);
            WindowAppearance.ApplyTitleBar(this, dark);
            chart.ApplyTheme(dark);
            chart.HighlightQuietHours = settings != null && settings.HighlightQuietHours;
            UpdatePeriodButtonColors();
#if WPF_UI
            if (historyHeader != null) historyHeader.SetTheme(dark, highContrast);
#endif
            if (snapshot != null) UpdateData(snapshot);
            Invalidate(true);
        }

        protected override void OnHandleCreated(EventArgs e)
        {
            base.OnHandleCreated(e);
            bool dark = !SystemInformation.HighContrast && (settings == null || AppSettings.IsDarkTheme(settings.Theme));
            WindowAppearance.ApplyTitleBar(this, dark);
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
            }
            base.Dispose(disposing);
            if (disposing)
            {
                remainingFont.Dispose();
                bodyFont.Dispose();
            }
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
                UpdatePeriodButtonColors();
                UpdateData(snapshot);
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
            RadioButton button = new PeriodButton();
            button.Text = text;
            button.AccessibleName = accessibleName;
            button.Appearance = Appearance.Button;
            button.FlatStyle = FlatStyle.Flat;
            button.FlatAppearance.BorderSize = 1;
            button.Size = new Size(104, 34);
            button.TextAlign = ContentAlignment.MiddleCenter;
            button.Margin = new Padding(0, 0, 2, 0);
            return button;
        }

        private void UpdatePeriodButtonColors()
        {
            bool highContrast = SystemInformation.HighContrast;
            bool dark = !highContrast && (settings == null || AppSettings.IsDarkTheme(settings.Theme));
            foreach (RadioButton button in new[] { weeklyButton, fiveHourButton })
            {
                button.BackColor = button.Checked ? (highContrast ? SystemColors.Highlight : Color.FromArgb(0, 105, 190)) :
                    highContrast ? SystemColors.Window : dark ? Color.FromArgb(31, 38, 48) : Color.White;
                button.ForeColor = button.Checked ? (highContrast ? SystemColors.HighlightText : Color.White) :
                    highContrast ? SystemColors.WindowText : dark ? Color.White : Color.Black;
                button.FlatAppearance.BorderColor = highContrast ? SystemColors.WindowText :
                    dark ? Color.FromArgb(79, 91, 107) : Color.FromArgb(170, 178, 188);
                button.FlatAppearance.CheckedBackColor = button.BackColor;
                button.Invalidate();
            }
        }

        private Color DividerColor()
        {
            return SystemInformation.HighContrast ? SystemColors.WindowText :
                settings == null || AppSettings.IsDarkTheme(settings.Theme) ? Color.FromArgb(73, 82, 93) : Color.FromArgb(195, 201, 210);
        }

        private static Label CreateSummaryValue()
        {
            Label label = new Label();
            label.Dock = DockStyle.Fill;
            label.AutoEllipsis = false;
            label.Margin = new Padding(0);
            label.TextAlign = ContentAlignment.MiddleLeft;
            return label;
        }

        private Control CreateSummaryBlock(string caption, Label value, string icon)
        {
            TableLayoutPanel block = new TableLayoutPanel();
            block.Dock = DockStyle.Fill;
            block.AutoSize = true;
            block.Margin = new Padding(0, 0, 12, 0);
            block.ColumnCount = string.IsNullOrEmpty(icon) ? 1 : 2;
            if (!string.IsNullOrEmpty(icon)) block.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 32));
            block.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            block.RowCount = 2;
            block.RowStyles.Add(new RowStyle(SizeType.Absolute, Math.Max(19, Font.Height + 2)));
            block.RowStyles.Add(new RowStyle(SizeType.Absolute, Math.Max(28, Math.Max(Font.Height * 2 + 4, value.Font.Height + 3))));
            Label label = CreateLabel(caption);
            label.Margin = Padding.Empty;
            value.AccessibleName = caption;
            int textColumn = 0;
            if (!string.IsNullOrEmpty(icon))
            {
                SummaryIcon image = new SummaryIcon(icon);
                image.Dock = DockStyle.Fill;
                block.Controls.Add(image, 0, 0);
                block.SetRowSpan(image, 2);
                textColumn = 1;
            }
            block.Controls.Add(label, textColumn, 0);
            block.Controls.Add(value, textColumn, 1);
            bool updatingHeight = false;
            EventHandler updateHeight = delegate
            {
                if (updatingHeight || label.Width <= 0 || value.Width <= 0) return;
                TextFormatFlags flags = TextFormatFlags.WordBreak | TextFormatFlags.TextBoxControl | TextFormatFlags.NoPrefix;
                int captionHeight = TextRenderer.MeasureText(label.Text, label.Font, new Size(label.Width, 32767), flags).Height + 2;
                int valueHeight = TextRenderer.MeasureText(value.Text, value.Font, new Size(value.Width, 32767), flags).Height + 2;
                captionHeight = Math.Max(19, captionHeight);
                valueHeight = Math.Max(Math.Max(28, Font.Height * 2 + 4), valueHeight);
                if ((int)block.RowStyles[0].Height == captionHeight && (int)block.RowStyles[1].Height == valueHeight) return;
                updatingHeight = true;
                try
                {
                    block.RowStyles[0].Height = captionHeight;
                    block.RowStyles[1].Height = valueHeight;
                }
                finally { updatingHeight = false; }
            };
            label.TextChanged += updateHeight;
            label.FontChanged += updateHeight;
            label.SizeChanged += updateHeight;
            value.TextChanged += updateHeight;
            value.FontChanged += updateHeight;
            value.SizeChanged += updateHeight;
            return block;
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

        private static GraphicsPath RoundedRectangle(RectangleF bounds, float radius)
        {
            GraphicsPath path = new GraphicsPath();
            float diameter = Math.Min(radius * 2, Math.Min(bounds.Width, bounds.Height));
            path.AddArc(bounds.Left, bounds.Top, diameter, diameter, 180, 90);
            path.AddArc(bounds.Right - diameter, bounds.Top, diameter, diameter, 270, 90);
            path.AddArc(bounds.Right - diameter, bounds.Bottom - diameter, diameter, diameter, 0, 90);
            path.AddArc(bounds.Left, bounds.Bottom - diameter, diameter, diameter, 90, 90);
            path.CloseFigure();
            return path;
        }

        private sealed class PeriodButton : RadioButton
        {
            public PeriodButton()
            {
                SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint |
                    ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
            }

            protected override void OnPaint(PaintEventArgs e)
            {
                if (SystemInformation.HighContrast)
                {
                    base.OnPaint(e);
                    return;
                }
                e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
                RectangleF bounds = new RectangleF(0.5f, 0.5f, Math.Max(1, Width - 1f), Math.Max(1, Height - 1f));
                using (GraphicsPath path = RoundedRectangle(bounds, 6 * e.Graphics.DpiX / 96f))
                using (Brush fill = new SolidBrush(BackColor))
                using (Pen border = new Pen(FlatAppearance.BorderColor))
                {
                    e.Graphics.FillPath(fill, path);
                    e.Graphics.DrawPath(border, path);
                }
                TextFormatFlags flags = TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter |
                    TextFormatFlags.SingleLine | TextFormatFlags.EndEllipsis;
                if (!ShowKeyboardCues) flags |= TextFormatFlags.HidePrefix;
                TextRenderer.DrawText(e.Graphics, Text, Font, ClientRectangle, ForeColor, flags);
                if (Focused && ShowFocusCues)
                    ControlPaint.DrawFocusRectangle(e.Graphics, Rectangle.Inflate(ClientRectangle, -4, -4), ForeColor, BackColor);
            }

            protected override void OnGotFocus(EventArgs e) { base.OnGotFocus(e); Invalidate(); }
            protected override void OnLostFocus(EventArgs e) { base.OnLostFocus(e); Invalidate(); }
        }

        private sealed class SummaryIcon : Control
        {
            private readonly string kind;

            public SummaryIcon(string kind)
            {
                this.kind = kind;
                TabStop = false;
                AccessibleRole = AccessibleRole.None;
                SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint |
                    ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
            }

            protected override void OnPaint(PaintEventArgs e)
            {
                base.OnPaint(e);
                e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
                bool dark = BackColor.GetBrightness() < 0.5f;
                Color accent = SystemInformation.HighContrast ? SystemColors.WindowText :
                    kind == "calendar" ? (dark ? Color.FromArgb(87, 185, 247) : Color.FromArgb(0, 104, 180)) :
                    (dark ? Color.FromArgb(242, 178, 77) : Color.FromArgb(160, 89, 0));
                float scale = Math.Min(Width / 32f, Height / 32f);
                float x = Math.Max(0, (Width - 23 * scale) / 2f - 2 * scale);
                float y = (Height - 23 * scale) / 2f;
                using (Pen pen = new Pen(accent, Math.Max(1.4f, 1.7f * scale)))
                {
                    pen.StartCap = LineCap.Round;
                    pen.EndCap = LineCap.Round;
                    pen.LineJoin = LineJoin.Round;
                    if (kind == "calendar")
                    {
                        e.Graphics.DrawRectangle(pen, x + 2 * scale, y + 5 * scale, 18 * scale, 16 * scale);
                        e.Graphics.DrawLine(pen, x + 2 * scale, y + 10 * scale, x + 20 * scale, y + 10 * scale);
                        e.Graphics.DrawLine(pen, x + 6 * scale, y + 2 * scale, x + 6 * scale, y + 7 * scale);
                        e.Graphics.DrawLine(pen, x + 16 * scale, y + 2 * scale, x + 16 * scale, y + 7 * scale);
                    }
                    else
                    {
                        e.Graphics.DrawLines(pen, new[] { new PointF(x + 2 * scale, y + 17 * scale),
                            new PointF(x + 8 * scale, y + 11 * scale), new PointF(x + 12 * scale, y + 14 * scale),
                            new PointF(x + 20 * scale, y + 5 * scale) });
                        e.Graphics.DrawLine(pen, x + 15 * scale, y + 5 * scale, x + 20 * scale, y + 5 * scale);
                        e.Graphics.DrawLine(pen, x + 20 * scale, y + 5 * scale, x + 20 * scale, y + 10 * scale);
                    }
                }
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
            private DateTime? selectedProjectionUtc;
            private RectangleF plot;

            public bool HighlightQuietHours { get; set; }

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
                DateTime? projectedTimestamp = weekly == showWeekly ? selectedProjectionUtc : null;
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
                if (samples.Count > 0)
                {
                    for (int i = samples.Count - 1; i > 0; i--)
                    {
                        if (UsageHistoryStore.IsReset(samples[i - 1], samples[i], weekly))
                        {
                            samples.RemoveRange(0, i);
                            break;
                        }
                    }
                }
                // Keep both ends of a plateau for its duration, but discard the
                // intermediate polls; only percentage changes are selectable.
                for (int i = samples.Count - 2; i > 0; i--)
                    if (!IsChangedReading(i) && !IsChangedReading(i + 1)) samples.RemoveAt(i);
                if (samples.Count > 0) startUtc = samples[0].TimestampUtc;
                UpdateEndUtc();
                int retainedIndex = -1;
                if (selectedTimestamp.HasValue)
                {
                    for (int i = 0; i < samples.Count; i++)
                    {
                        if (samples[i].TimestampUtc == selectedTimestamp.Value &&
                            IsChangedReading(i))
                        {
                            retainedIndex = i;
                            break;
                        }
                    }
                }
                selectedIndex = -1;
                SelectSample(retainedIndex);
                selectedProjectionUtc = projectedTimestamp;
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
                UpdateEndUtc();
                if (selectedProjectionUtc.HasValue)
                {
                    DateTime timestamp = selectedProjectionUtc.Value;
                    if (projection != null && timestamp >= projection.StartUtc && timestamp <= endUtc)
                        SelectProjection(timestamp);
                    else SelectSample(-1);
                }
                Invalidate();
            }

            private void UpdateEndUtc()
            {
                endUtc = observedEndUtc;
                if (samples.Count > 0)
                {
                    UsageHistoryWindow window = UsageHistoryStore.GetWindow(samples[samples.Count - 1], weekly);
                    if (window.ResetAtUtc.HasValue && window.ResetAtUtc.Value > endUtc)
                        endUtc = window.ResetAtUtc.Value;
                }
                if (projection != null && projection.EndUtc > endUtc) endUtc = projection.EndUtc;
                if (endUtc <= startUtc) endUtc = startUtc.AddMinutes(1);
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
                    dark ? Color.FromArgb(65, 74, 86) : Color.FromArgb(195, 201, 210);
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
                    if (HighlightQuietHours && projection.UsesInactiveHours && HasProjectedConsumption(projection.Points))
                    {
                        UsageProjectionPoint previous = null;
                        using (Brush quiet = new SolidBrush(Color.FromArgb(24, line)))
                        foreach (UsageProjectionPoint point in projection.Points)
                        {
                            if (previous != null && previous.RemainingPercent > 0 &&
                                Math.Abs(previous.RemainingPercent - point.RemainingPercent) < 0.000001 &&
                                point.TimestampUtc > previous.TimestampUtc)
                            {
                                float left = Math.Max(plot.Left, GetChartPoint(previous.TimestampUtc, 0).X);
                                float right = Math.Min(plot.Right, GetChartPoint(point.TimestampUtc, 0).X);
                                if (right > left) graphics.FillRectangle(quiet, left, plot.Top, right - left, plot.Height);
                            }
                            previous = point;
                        }
                    }
                    float recordedLeft = plot.Left + 25 * scale;
                    float recordedWidth = projectedStart.X - recordedLeft - 24 * scale;
                    if (recordedWidth > 55 * scale)
                        TextRenderer.DrawText(graphics, "Recorded", Font,
                            Rectangle.Round(new RectangleF(recordedLeft, 0, recordedWidth, labelHeight)), line,
                            TextFormatFlags.NoPadding | TextFormatFlags.NoPrefix | TextFormatFlags.SingleLine |
                            TextFormatFlags.VerticalCenter | TextFormatFlags.HorizontalCenter);
                    if (projectedStart.X > plot.Left + 115 * scale && projectedStart.X < plot.Right - 115 * scale)
                    {
                        Rectangle markerBounds = Rectangle.Round(new RectangleF(projectedStart.X - 20 * scale, 0,
                            40 * scale, labelHeight));
                        using (GraphicsPath markerPath = RoundedRectangle(markerBounds, 5 * scale))
                        using (Brush marker = new SolidBrush(SystemInformation.HighContrast ? SystemColors.Highlight : Color.FromArgb(0, 112, 205)))
                        using (Pen markerBorder = new Pen(SystemInformation.HighContrast ? SystemColors.HighlightText : Color.FromArgb(87, 185, 247)))
                        {
                            graphics.FillPath(marker, markerPath);
                            graphics.DrawPath(markerBorder, markerPath);
                            graphics.FillPolygon(marker, new[] { new PointF(projectedStart.X - 4 * scale, markerBounds.Bottom - 1),
                                new PointF(projectedStart.X + 4 * scale, markerBounds.Bottom - 1),
                                new PointF(projectedStart.X, plot.Top) });
                        }
                        TextRenderer.DrawText(graphics, "Now", Font, markerBounds,
                            SystemInformation.HighContrast ? SystemColors.HighlightText : Color.White,
                            TextFormatFlags.NoPadding | TextFormatFlags.NoPrefix | TextFormatFlags.SingleLine |
                            TextFormatFlags.VerticalCenter | TextFormatFlags.HorizontalCenter);
                    }
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
                    if (samples.Count > 0)
                    {
                        float width = Math.Max(AxisLabelWidth(graphics, startUtc, showDates),
                            AxisLabelWidth(graphics, endUtc, showDates));
                        width = Math.Max(width, TextRenderer.MeasureText(graphics, "11:59 PM", Font,
                            Size.Empty, TextFormatFlags.NoPadding).Width) + 4 * scale;
                        List<DateTime> ticks = GetAxisTicks(width + 16 * scale);
                        float previousRight = plot.Left - 12 * scale;
                        float lastLeft = plot.Right - AxisLabelWidth(graphics, endUtc, showDates) - 4 * scale;
                        for (int i = 0; i < ticks.Count; i++)
                        {
                            float x = GetChartPoint(ticks[i], 0).X;
                            float labelWidth = AxisLabelWidth(graphics, ticks[i], showDates) + 4 * scale;
                            bool first = i == 0;
                            bool last = i == ticks.Count - 1;
                            float left = first ? plot.Left : last ? plot.Right - labelWidth : x - labelWidth / 2;
                            if (!first && !last && (left < previousRight + 12 * scale ||
                                left + labelWidth > lastLeft - 12 * scale)) continue;
                            graphics.DrawLine(gridPen, x, plot.Bottom, x, plot.Bottom + 4 * scale);
                            DateTime local = ticks[i].ToLocalTime();
                            Rectangle bounds = Rectangle.Round(new RectangleF(left, plot.Bottom + 6 * scale,
                                labelWidth, labelHeight));
                            TextFormatFlags flags = TextFormatFlags.NoPadding | TextFormatFlags.NoPrefix |
                                TextFormatFlags.SingleLine | TextFormatFlags.VerticalCenter |
                                (first ? TextFormatFlags.Left : last ? TextFormatFlags.Right : TextFormatFlags.HorizontalCenter);
                            TextRenderer.DrawText(graphics, local.ToString(showDates ? "MMM d" : "t",
                                CultureInfo.CurrentCulture), Font, bounds, muted, flags);
                            if (showDates)
                            {
                                bounds.Y += (int)Math.Ceiling(labelHeight);
                                TextRenderer.DrawText(graphics, local.ToString("t", CultureInfo.CurrentCulture),
                                    Font, bounds, muted, flags);
                            }
                            previousRight = left + labelWidth;
                        }
                    }
                }

                if (projection != null)
                {
                    PointF projectedStart = GetChartPoint(projection.StartUtc, projection.StartRemainingPercent);
                    PointF projectedEnd = GetChartPoint(projection.EndUtc, projection.EndRemainingPercent);
                    using (Pen nowPen = new Pen(line, Math.Max(1, scale)))
                    using (Brush marker = new SolidBrush(line))
                    {
                        graphics.DrawLine(nowPen, projectedStart.X, plot.Top, projectedStart.X, plot.Bottom);
                        graphics.FillEllipse(marker, projectedStart.X - 5 * scale, projectedStart.Y - 5 * scale, 10 * scale, 10 * scale);
                        using (Pen outline = new Pen(SystemInformation.HighContrast ? SystemColors.WindowText : Color.White, Math.Max(1, scale)))
                            graphics.DrawEllipse(outline, projectedStart.X - 5 * scale, projectedStart.Y - 5 * scale, 10 * scale, 10 * scale);
                    }
                    using (Pen projectionPen = new Pen(projectionColor, Math.Max(1.5f, 2 * scale)))
                    {
                        projectionPen.DashStyle = DashStyle.Dash;
                        List<PointF> projectedPoints = new List<PointF> { projectedStart };
                        if (projection.Points != null)
                        {
                            foreach (UsageProjectionPoint point in projection.Points)
                                projectedPoints.Add(GetChartPoint(point.TimestampUtc, point.RemainingPercent));
                        }
                        else projectedPoints.Add(projectedEnd);
                        if (projection.EndRemainingPercent <= 0 && endUtc > projection.EndUtc)
                            projectedPoints.Add(GetChartPoint(endUtc, 0));
                        if (projectedPoints.Count > 1) graphics.DrawLines(projectionPen, projectedPoints.ToArray());
                        projectionPen.DashStyle = DashStyle.Solid;
                        graphics.DrawEllipse(projectionPen, projectedEnd.X - 3 * scale, projectedEnd.Y - 3 * scale, 6 * scale, 6 * scale);
                        if (selectedProjectionUtc.HasValue)
                        {
                            PointF selected = GetChartPoint(selectedProjectionUtc.Value, ProjectedRemaining(selectedProjectionUtc.Value));
                            graphics.DrawLine(projectionPen, selected.X, plot.Top, selected.X, plot.Bottom);
                            graphics.DrawEllipse(projectionPen, selected.X - 4 * scale, selected.Y - 4 * scale, 8 * scale, 8 * scale);
                        }
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
                        else if (i == 0 || ((i == samples.Count - 1 || i == selectedIndex) && IsChangedReading(i)))
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
                if (projection != null && e.X > GetChartPoint(projection.StartUtc, projection.StartRemainingPercent).X)
                {
                    double fraction = Math.Max(0, Math.Min(1, (e.X - plot.Left) / plot.Width));
                    DateTime timestamp = startUtc.AddSeconds((endUtc - startUtc).TotalSeconds * fraction);
                    SelectProjection(timestamp);
                    return;
                }
                int nearest = -1;
                double distance = double.MaxValue;
                for (int i = 0; i < samples.Count; i++)
                {
                    UsageHistoryWindow window = UsageHistoryStore.GetWindow(samples[i], weekly);
                    if (window == null || !IsChangedReading(i))
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
                while (next >= 0 && next < samples.Count && !IsChangedReading(next));
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
                if (selectedIndex < 0 && !selectedProjectionUtc.HasValue && samples.Count > 0)
                {
                    int index = samples.Count - 1;
                    while (index > 0 && !IsChangedReading(index)) index--;
                    SelectSample(index);
                }
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

            private static bool HasProjectedConsumption(List<UsageProjectionPoint> points)
            {
                if (points == null) return false;
                for (int i = 1; i < points.Count; i++)
                    if (points[i - 1].RemainingPercent - points[i].RemainingPercent > 0.000001) return true;
                return false;
            }

            private void SelectSample(int index)
            {
                if (index == selectedIndex && index >= 0 && !selectedProjectionUtc.HasValue)
                {
                    return;
                }
                selectedIndex = index;
                selectedProjectionUtc = null;
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

            private bool IsChangedReading(int index)
            {
                if (index == 0) return true;
                UsageHistoryWindow before = UsageHistoryStore.GetWindow(samples[index - 1], weekly);
                UsageHistoryWindow after = UsageHistoryStore.GetWindow(samples[index], weekly);
                return Math.Abs(after.UsedPercent - before.UsedPercent) > 0.001 ||
                    UsageHistoryStore.IsReset(samples[index - 1], samples[index], weekly);
            }

            private double ProjectedRemaining(DateTime timestamp)
            {
                if (timestamp >= projection.EndUtc) return projection.EndRemainingPercent;
                DateTime before = projection.StartUtc;
                double remaining = projection.StartRemainingPercent;
                if (projection.Points != null)
                {
                    foreach (UsageProjectionPoint point in projection.Points)
                    {
                        if (point.TimestampUtc <= before) continue;
                        if (timestamp <= point.TimestampUtc)
                            return remaining + (point.RemainingPercent - remaining) *
                                (timestamp - before).TotalSeconds / (point.TimestampUtc - before).TotalSeconds;
                        before = point.TimestampUtc;
                        remaining = point.RemainingPercent;
                    }
                }
                return remaining + (projection.EndRemainingPercent - remaining) *
                    (timestamp - before).TotalSeconds / Math.Max(1, (projection.EndUtc - before).TotalSeconds);
            }

            private void SelectProjection(DateTime timestamp)
            {
                selectedIndex = -1;
                selectedProjectionUtc = timestamp;
                double remaining = Math.Round(Math.Max(0, Math.Min(100, ProjectedRemaining(timestamp))),
                    MidpointRounding.AwayFromZero);
                SelectedDetail = "Projected: " + timestamp.ToLocalTime().ToString("G", CultureInfo.CurrentCulture) +
                    Environment.NewLine + "\u2248" + remaining.ToString("0", CultureInfo.CurrentCulture) +
                    "% remaining  |  \u2248" + (100 - remaining).ToString("0", CultureInfo.CurrentCulture) + "% used";
                AccessibleDescription = SelectedDetail;
                EventHandler handler = SelectionChanged;
                if (handler != null) handler(this, EventArgs.Empty);
                Invalidate();
            }

            private PointF GetPoint(UsageHistorySample sample, UsageHistoryWindow window)
            {
                return GetChartPoint(sample.TimestampUtc, 100 - window.UsedPercent);
            }

            private List<DateTime> GetAxisTicks(float minimumSpacing)
            {
                List<DateTime> ticks = new List<DateTime> { startUtc };
                int intervals = Math.Max(1, Math.Min(8, (int)(plot.Width / minimumSpacing)));
                double targetMinutes = (endUtc - startUtc).TotalMinutes / intervals;
                int[] steps = { 1, 5, 15, 30, 60, 180, 360, 720, 1440, 2880, 10080, 20160, 40320 };
                int stepMinutes = steps[steps.Length - 1];
                foreach (int step in steps)
                {
                    if (step >= targetMinutes) { stepMinutes = step; break; }
                }
                long stepTicks = TimeSpan.FromMinutes(stepMinutes).Ticks;
                DateTime local = startUtc.ToLocalTime();
                long nextTicks = local.Ticks - local.Ticks % stepTicks;
                while (nextTicks <= DateTime.MaxValue.Ticks - stepTicks)
                {
                    nextTicks += stepTicks;
                    DateTime next = new DateTime(nextTicks, DateTimeKind.Unspecified);
                    if (TimeZoneInfo.Local.IsInvalidTime(next)) continue;
                    DateTime utc = TimeZoneInfo.ConvertTimeToUtc(next);
                    if (utc >= endUtc) break;
                    if (utc > startUtc) ticks.Add(utc);
                }
                ticks.Add(endUtc);
                return ticks;
            }

            private float AxisLabelWidth(Graphics graphics, DateTime timestamp, bool showDates)
            {
                DateTime local = timestamp.ToLocalTime();
                float width = TextRenderer.MeasureText(graphics, local.ToString("t", CultureInfo.CurrentCulture),
                    Font, Size.Empty, TextFormatFlags.NoPadding).Width;
                if (showDates)
                    width = Math.Max(width, TextRenderer.MeasureText(graphics,
                        local.ToString("MMM d", CultureInfo.CurrentCulture), Font, Size.Empty, TextFormatFlags.NoPadding).Width);
                return width;
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
