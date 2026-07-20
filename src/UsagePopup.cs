using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Globalization;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal sealed class UsagePopup : Form
    {
        private const int WmDpiChanged = 0x02E0;
        private const int LogicalWidth = 350;
        private const int LogicalUsageTop = 43;
        private const int LogicalUsageRowHeight = 48;
        private const int LogicalFooterTopGap = 5;
        private const int LogicalContentBottomPadding = 2;
        private const int LogicalFooterLineHeight = 18;
        private const int LogicalDetailsButtonHeight = 22;
        private const int LogicalFooterBottomPadding = 5;
        private const string RefreshGlyph = "\uE72C";

        private readonly Button refreshButton;
        private readonly Button settingsButton;
        private readonly Button detailsButton;
        private readonly ToolTip actionToolTip;
        private readonly Timer refreshAnimationTimer;
        private readonly Font titleFont;
        private readonly Font rowLabelFont;
        private readonly Font percentFont;
        private readonly Font detailFont;
        private readonly Font detailHeadingFont;
        private readonly Font glyphFont;

        private AppSettings settings;
        private UsageSnapshot snapshot;
        private int currentDpi = 96;
        private int refreshAnimationAngle = -90;
        private bool refreshing;
        private bool detailsExpanded;
        private bool updatingLayout;

        public event EventHandler RefreshRequested;
        public event EventHandler SettingsRequested;

        public UsagePopup(AppSettings settings)
        {
            this.settings = settings;

            titleFont = new Font("Segoe UI", 10.0f, FontStyle.Bold, GraphicsUnit.Point);
            rowLabelFont = new Font("Segoe UI", 9.0f, FontStyle.Regular, GraphicsUnit.Point);
            percentFont = new Font("Segoe UI", 8.0f, FontStyle.Bold, GraphicsUnit.Point);
            detailFont = new Font("Segoe UI", 8.0f, FontStyle.Regular, GraphicsUnit.Point);
            detailHeadingFont = new Font("Segoe UI", 8.0f, FontStyle.Bold, GraphicsUnit.Point);
            glyphFont = new Font("Segoe MDL2 Assets", 10.0f, FontStyle.Regular, GraphicsUnit.Point);

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
            detailsButton = CreateDetailsButton();
            refreshButton.Click += RefreshButton_Click;
            refreshButton.Paint += RefreshButton_Paint;
            settingsButton.Click += SettingsButton_Click;
            detailsButton.Click += DetailsButton_Click;
            detailsButton.Paint += DetailsButton_Paint;
            Controls.Add(refreshButton);
            Controls.Add(settingsButton);
            Controls.Add(detailsButton);

            actionToolTip = new ToolTip();
            actionToolTip.AutomaticDelay = 350;
            actionToolTip.AutoPopDelay = 5000;
            actionToolTip.ShowAlways = true;
            actionToolTip.SetToolTip(refreshButton, "Refresh usage");
            actionToolTip.SetToolTip(settingsButton, "Open settings");
            actionToolTip.SetToolTip(detailsButton, "Show limit resets");

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
            snapshot = value;
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

        public void ShowNear(Point cursor)
        {
            detailsExpanded = false;
            UpdateLayoutMetrics(false);

            Rectangle area = Screen.FromPoint(cursor).WorkingArea;
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

            return base.ProcessCmdKey(ref msg, keyData);
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

            using (Pen borderPen = new Pen(borderColor, Math.Max(1.0f, currentDpi / 96.0f)))
            {
                graphics.DrawRectangle(borderPen, 0, 0, ClientSize.Width - 1, ClientSize.Height - 1);
                graphics.DrawLine(
                    borderPen,
                    ScaleMetric(10),
                    ScaleMetric(38),
                    ClientSize.Width - ScaleMetric(10),
                    ScaleMetric(38));
            }

            DrawHeader(graphics, textColor, mutedColor);

            DateTime lastUpdated = snapshot != null ? snapshot.LastUpdated : DateTime.MinValue;
            int rowY = LogicalUsageTop;
            if (snapshot != null && snapshot.Weekly != null)
            {
                DrawUsageRow(
                    graphics,
                    "Weekly",
                    snapshot.Weekly,
                    lastUpdated,
                    ScaleMetric(14),
                    ScaleMetric(rowY),
                    textColor,
                    mutedColor);
                rowY += LogicalUsageRowHeight;
            }
            if (snapshot != null && snapshot.FiveHour != null)
            {
                DrawUsageRow(
                    graphics,
                    "5-hour",
                    snapshot.FiveHour,
                    lastUpdated,
                    ScaleMetric(14),
                    ScaleMetric(rowY),
                    textColor,
                    mutedColor);
            }

            if (!string.IsNullOrEmpty(BuildStatusLine()) || HasExpandableDetails())
            {
                DrawFooter(graphics, textColor, mutedColor, borderColor, dark);
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

        private Button CreateDetailsButton()
        {
            Button button = new Button();
            button.Name = "detailsButton";
            button.Text = "Limit resets";
            button.Font = detailFont;
            button.AccessibleName = "Show limit resets";
            button.AccessibleDescription = "Show available limit reset expirations";
            button.AccessibleRole = AccessibleRole.PushButton;
            button.TabIndex = 2;
            button.TabStop = true;
            button.AutoSize = false;
            button.Margin = new Padding(0);
            button.Padding = new Padding(4, 0, 24, 0);
            button.FlatStyle = FlatStyle.Flat;
            button.FlatAppearance.BorderSize = 0;
            button.UseVisualStyleBackColor = false;
            button.UseCompatibleTextRendering = false;
            button.TextAlign = ContentAlignment.MiddleLeft;
            button.Cursor = Cursors.Hand;
            button.Visible = false;
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

            float scale = currentDpi / 96.0f;
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

        private void DetailsButton_Click(object sender, EventArgs e)
        {
            if (!HasExpandableDetails())
            {
                return;
            }

            detailsExpanded = !detailsExpanded;
            UpdateLayoutMetrics(true);
            Invalidate(true);
        }

        private void DetailsButton_Paint(object sender, PaintEventArgs e)
        {
            string glyph = detailsExpanded ? "\uE70E" : "\uE70D";
            Rectangle glyphBounds = new Rectangle(
                detailsButton.ClientSize.Width - ScaleMetric(24),
                0,
                ScaleMetric(20),
                detailsButton.ClientSize.Height);
            TextRenderer.DrawText(
                e.Graphics,
                glyph,
                glyphFont,
                glyphBounds,
                detailsButton.ForeColor,
                TextFormatFlags.SingleLine |
                    TextFormatFlags.VerticalCenter |
                    TextFormatFlags.HorizontalCenter |
                    TextFormatFlags.NoPadding);
        }

        private void DrawHeader(Graphics graphics, Color textColor, Color mutedColor)
        {
            int left = ScaleMetric(14);
            int top = ScaleMetric(9);
            int right = refreshButton.Left - ScaleMetric(8);
            int height = ScaleMetric(22);
            Rectangle titleBounds = new Rectangle(left, top, Math.Max(0, right - left), height);
            TextFormatFlags flags = TextFormatFlags.SingleLine |
                TextFormatFlags.VerticalCenter |
                TextFormatFlags.NoPrefix |
                TextFormatFlags.NoPadding;

            const string title = "Codex Usage";
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
            TextRenderer.DrawText(graphics, title, titleFont, titleTextBounds, textColor, flags);

            string headerDetail = null;
            if ((settings == null || settings.ShowPopupLastUpdated) &&
                snapshot != null && snapshot.LastUpdated != DateTime.MinValue)
            {
                headerDetail = "Updated " + TimeFormatter.FormatClock(snapshot.LastUpdated);
            }

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

            int width = ClientSize.Width - x - ScaleMetric(14);
            string resetDetail = "";
            if (settings == null || settings.ShowPopupResetTimes)
            {
                resetDetail = window.ResetAfterSeconds.HasValue
                    ? "Next reset: " + TimeFormatter.FormatResetDateTime(lastUpdated, window.ResetAfterSeconds.Value) +
                        " (" + TimeFormatter.FormatDuration(window.ResetAfterSeconds.Value) + ")"
                    : "Next reset: ?";
            }

            Rectangle headerBounds = new Rectangle(x, y, width, ScaleMetric(18));
            TextFormatFlags singleLine = TextFormatFlags.SingleLine |
                TextFormatFlags.VerticalCenter |
                TextFormatFlags.NoPrefix |
                TextFormatFlags.NoPadding;
            DrawUsageHeader(
                graphics,
                label,
                resetDetail,
                headerBounds,
                textColor,
                mutedColor,
                singleLine);

            Rectangle barBounds = new Rectangle(x, y + ScaleMetric(20), width, ScaleMetric(20));
            bool dark = IsDarkTheme();
            Color trackColor = GetTrackColor(dark);
            using (Brush trackBrush = new SolidBrush(trackColor))
            {
                graphics.FillRectangle(trackBrush, barBounds);
            }

            int remaining = window.RemainingPercent;
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
                using (Brush fillBrush = new SolidBrush(fillColor))
                {
                    graphics.FillRectangle(
                        fillBrush,
                        new Rectangle(barBounds.X, barBounds.Y, fillWidth, barBounds.Height));
                }
            }

            Color percentBackground = fillWidth >= (barBounds.Width / 2) ? fillColor : trackColor;
            Color percentColor = GetContrastingTextColor(percentBackground);
            if (SystemInformation.HighContrast)
            {
                percentColor = fillWidth >= (barBounds.Width / 2)
                    ? SystemColors.HighlightText
                    : SystemColors.WindowText;
            }
            TextRenderer.DrawText(
                graphics,
                remaining.ToString(CultureInfo.CurrentCulture) + "%",
                percentFont,
                barBounds,
                percentColor,
                singleLine | TextFormatFlags.HorizontalCenter);
        }

        private void DrawUsageHeader(
            Graphics graphics,
            string label,
            string resetDetail,
            Rectangle bounds,
            Color textColor,
            Color mutedColor,
            TextFormatFlags flags)
        {
            Size labelSize = TextRenderer.MeasureText(
                graphics,
                label,
                rowLabelFont,
                new Size(32767, bounds.Height),
                flags);
            int labelWidth = Math.Min(bounds.Width, labelSize.Width);
            Rectangle labelBounds = new Rectangle(bounds.X, bounds.Y, labelWidth, bounds.Height);
            TextRenderer.DrawText(
                graphics,
                label,
                rowLabelFont,
                labelBounds,
                textColor,
                flags | TextFormatFlags.EndEllipsis);

            if (string.IsNullOrEmpty(resetDetail))
            {
                return;
            }

            int gap = ScaleMetric(12);
            int resetLeft = Math.Min(bounds.Right, labelBounds.Right + gap);
            Rectangle resetBounds = new Rectangle(
                resetLeft,
                bounds.Y,
                Math.Max(0, bounds.Right - resetLeft),
                bounds.Height);
            TextRenderer.DrawText(
                graphics,
                resetDetail,
                detailFont,
                resetBounds,
                mutedColor,
                flags | TextFormatFlags.Right | TextFormatFlags.EndEllipsis);
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
                TextFormatFlags.EndEllipsis;

            if (string.IsNullOrEmpty(leftText))
            {
                if (!string.IsNullOrEmpty(rightText))
                {
                    TextRenderer.DrawText(
                        graphics,
                        rightText,
                        detailFont,
                        bounds,
                        color,
                        drawFlags | TextFormatFlags.Right);
                }
                return;
            }

            if (string.IsNullOrEmpty(rightText))
            {
                TextRenderer.DrawText(graphics, leftText, detailFont, bounds, color, drawFlags);
                return;
            }

            Size leftSize = TextRenderer.MeasureText(
                graphics,
                leftText,
                detailFont,
                new Size(32767, bounds.Height),
                measureFlags);
            Size rightSize = TextRenderer.MeasureText(
                graphics,
                rightText,
                detailFont,
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
                TextRenderer.DrawText(graphics, leftText, detailFont, leftBounds, color, drawFlags);
                TextRenderer.DrawText(
                    graphics,
                    rightText,
                    detailFont,
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
            TextRenderer.DrawText(graphics, leftText, detailFont, constrainedLeft, color, drawFlags);
            TextRenderer.DrawText(
                graphics,
                rightText,
                detailFont,
                constrainedRight,
                color,
                drawFlags | TextFormatFlags.Right);
        }

        private void DrawFooter(
            Graphics graphics,
            Color textColor,
            Color mutedColor,
            Color borderColor,
            bool dark)
        {
            int footerContentTop = GetLogicalFooterContentTop();
            int dividerY = ScaleMetric(footerContentTop - 6);
            if (GetVisibleUsageRowCount() > 0)
            {
                using (Pen dividerPen = new Pen(borderColor, Math.Max(1.0f, currentDpi / 96.0f)))
                {
                    graphics.DrawLine(
                        dividerPen,
                        ScaleMetric(10),
                        dividerY,
                        ClientSize.Width - ScaleMetric(10),
                        dividerY);
                }
            }

            int y = ScaleMetric(footerContentTop);
            TextFormatFlags flags = TextFormatFlags.SingleLine |
                TextFormatFlags.VerticalCenter |
                TextFormatFlags.EndEllipsis |
                TextFormatFlags.NoPrefix |
                TextFormatFlags.NoPadding;

            string statusLine = BuildStatusLine();
            if (!string.IsNullOrEmpty(statusLine))
            {
                Rectangle statusBounds = new Rectangle(
                    ScaleMetric(14),
                    y,
                    ClientSize.Width - ScaleMetric(28),
                    ScaleMetric(17));
                TextRenderer.DrawText(
                    graphics,
                    statusLine,
                    detailFont,
                    statusBounds,
                    GetStatusColor(dark),
                    flags);
                y += ScaleMetric(LogicalFooterLineHeight);
            }

            if (detailsButton.Visible && detailsExpanded)
            {
                y = detailsButton.Bottom + ScaleMetric(4);
                DrawExpandedDetails(graphics, y, textColor, mutedColor);
            }
        }

        private void DrawExpandedDetails(
            Graphics graphics,
            int y,
            Color textColor,
            Color mutedColor)
        {
            if (ShouldShowResetAvailability() && HasResetInformation())
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

                for (int index = 0; index < knownCount; index++)
                {
                    RateLimitResetCredit credit = snapshot.AvailableResets[index];
                    string title = credit != null
                        ? CompactSingleLine(credit.Title, 42)
                        : null;
                    if (string.IsNullOrEmpty(title))
                    {
                        title = "Reset";
                    }

                    string expiration = credit != null && credit.ExpiresAtUtc.HasValue
                        ? "Expires " + TimeFormatter.FormatDateTime(
                            credit.ExpiresAtUtc.Value.ToLocalTime())
                        : "Expiration unavailable";
                    DrawMeasuredDetails(
                        graphics,
                        (index + 1).ToString(CultureInfo.CurrentCulture) + ". " + title,
                        expiration,
                        CreateDetailBounds(y),
                        mutedColor);
                    y += ScaleMetric(LogicalFooterLineHeight);
                }

                int unitemizedCount = Math.Max(0, displayCount - knownCount);
                if (knownCount == 0 || unitemizedCount > 0)
                {
                    string label = displayCount == 0
                        ? "No resets available"
                        : unitemizedCount.ToString(CultureInfo.CurrentCulture) +
                            (unitemizedCount == 1 ? " additional reset" : " additional resets");
                    string detail = displayCount == 0 ? "" : "Expiration unavailable";
                    DrawMeasuredDetails(
                        graphics,
                        label,
                        detail,
                        CreateDetailBounds(y),
                        mutedColor);
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
                    ClientSize.Width - ScaleMetric(28),
                    ScaleMetric(17)),
                color,
                TextFormatFlags.SingleLine |
                    TextFormatFlags.VerticalCenter |
                    TextFormatFlags.EndEllipsis |
                    TextFormatFlags.NoPrefix |
                    TextFormatFlags.NoPadding);
        }

        private Rectangle CreateDetailBounds(int y)
        {
            int left = ScaleMetric(22);
            return new Rectangle(
                left,
                y,
                ClientSize.Width - left - ScaleMetric(14),
                ScaleMetric(17));
        }

        private bool HasExpandableDetails()
        {
            return ShouldShowResetAvailability() && HasResetInformation();
        }

        private bool ShouldShowResetAvailability()
        {
            return settings == null || settings.ShowResetAvailability;
        }

        private bool HasResetInformation()
        {
            return snapshot != null &&
                (snapshot.AvailableResetCount.HasValue ||
                    (snapshot.AvailableResets != null && snapshot.AvailableResets.Count > 0));
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

        private int GetExpandedDetailLineCount()
        {
            int lineCount = 0;
            if (ShouldShowResetAvailability() && HasResetInformation())
            {
                int knownCount = snapshot.AvailableResets != null
                    ? snapshot.AvailableResets.Count
                    : 0;
                int displayCount = GetResetDisplayCount();
                lineCount += 1 + knownCount;
                if (knownCount == 0 || displayCount > knownCount)
                {
                    lineCount++;
                }
            }

            return lineCount;
        }

        private string BuildStatusLine()
        {
            if (snapshot == null)
            {
                return "Checking limits...";
            }

            string providedStatus = CompactSingleLine(snapshot.StatusMessage, 120);
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
                    : CompactSingleLine(snapshot.ErrorMessage, 120);
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
            if (updatingLayout)
            {
                return;
            }

            updatingLayout = true;
            try
            {
                int oldBottom = Bottom;
                bool hasStatus = !string.IsNullOrEmpty(BuildStatusLine());
                bool hasDetails = HasExpandableDetails();
                if (!hasDetails)
                {
                    detailsExpanded = false;
                }

                int footerContentTop = GetLogicalFooterContentTop();
                int logicalHeight = footerContentTop + LogicalContentBottomPadding;
                if (hasStatus || hasDetails)
                {
                    logicalHeight = footerContentTop;
                    if (hasStatus)
                    {
                        logicalHeight += LogicalFooterLineHeight;
                    }
                    if (hasDetails)
                    {
                        logicalHeight += LogicalDetailsButtonHeight;
                        if (detailsExpanded)
                        {
                            logicalHeight += 4 +
                                (GetExpandedDetailLineCount() * LogicalFooterLineHeight);
                        }
                    }
                    logicalHeight += LogicalFooterBottomPadding;
                }
                Size desiredSize = new Size(ScaleMetric(LogicalWidth), ScaleMetric(logicalHeight));
                if (ClientSize != desiredSize)
                {
                    ClientSize = desiredSize;
                }

                LayoutActionButtons();
                LayoutFooterControls(hasStatus, hasDetails);
                UpdateRegion();

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
            int size = ScaleMetric(30);
            int top = ScaleMetric(5);
            int right = ScaleMetric(8);
            int gap = ScaleMetric(2);
            settingsButton.Bounds = new Rectangle(ClientSize.Width - right - size, top, size, size);
            refreshButton.Bounds = new Rectangle(settingsButton.Left - gap - size, top, size, size);
        }

        private void LayoutFooterControls(bool hasStatus, bool hasDetails)
        {
            detailsButton.Visible = hasDetails;
            if (!hasDetails)
            {
                return;
            }

            int y = GetLogicalFooterContentTop() + (hasStatus ? LogicalFooterLineHeight : 0);
            detailsButton.Bounds = new Rectangle(
                ScaleMetric(10),
                ScaleMetric(y),
                ClientSize.Width - ScaleMetric(20),
                ScaleMetric(LogicalDetailsButtonHeight));
            detailsButton.AccessibleName = detailsExpanded
                ? "Hide limit resets"
                : "Show limit resets";
            detailsButton.AccessibleDescription = detailsButton.AccessibleName;
            actionToolTip.SetToolTip(detailsButton, detailsButton.AccessibleName);
            detailsButton.Invalidate();
        }

        private int GetLogicalFooterContentTop()
        {
            return LogicalUsageTop +
                (GetVisibleUsageRowCount() * LogicalUsageRowHeight) +
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
            refreshButton.AccessibleName = active ? "Refreshing usage" : "Refresh usage";
            refreshButton.AccessibleDescription = refreshButton.AccessibleName;
            actionToolTip.SetToolTip(refreshButton, active ? "Refreshing usage" : "Refresh usage");
            refreshButton.Invalidate();
        }

        private bool IsRefreshActive()
        {
            return refreshing || (snapshot != null && snapshot.IsRefreshing);
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
            ApplyButtonColors(refreshButton, backColor, textColor, hoverColor, pressedColor);
            ApplyButtonColors(settingsButton, backColor, textColor, hoverColor, pressedColor);
            ApplyButtonColors(detailsButton, backColor, textColor, hoverColor, pressedColor);
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
            button.FlatAppearance.BorderColor = backColor;
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
            return dark ? Color.FromArgb(54, 54, 54) : Color.FromArgb(245, 245, 245);
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
            return dark ? Color.FromArgb(82, 82, 82) : Color.FromArgb(185, 185, 185);
        }

        private static Color GetTrackColor(bool dark)
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.ControlDark;
            }
            return dark ? Color.FromArgb(22, 22, 22) : Color.FromArgb(210, 210, 210);
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

        private int ScaleMetric(int logicalPixels)
        {
            return (int)Math.Round(logicalPixels * (currentDpi / 96.0));
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
