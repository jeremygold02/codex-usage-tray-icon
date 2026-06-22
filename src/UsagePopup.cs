using System;
using System.Collections.Generic;
using System.Collections.Specialized;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.IO;
using System.Net;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal sealed class UsagePopup : Form
    {
        private AppSettings settings;
        private UsageSnapshot snapshot;
        private Rectangle refreshBounds;
        private Rectangle settingsBounds;
        private bool refreshHover;
        private bool settingsHover;

        public event EventHandler RefreshRequested;
        public event EventHandler SettingsRequested;

        public UsagePopup(AppSettings settings)
        {
            this.settings = settings;
            Text = "Codex Usage";
            FormBorderStyle = FormBorderStyle.None;
            ShowInTaskbar = false;
            TopMost = true;
            StartPosition = FormStartPosition.Manual;
            ClientSize = new Size(390, 184);
            ApplyThemeColors();
            DoubleBuffered = true;
            Padding = new Padding(0);
            UpdateRegion();
        }

        public void ApplySettings(AppSettings value)
        {
            settings = value;
            ApplyThemeColors();
            Invalidate();
        }

        public void UpdateSnapshot(UsageSnapshot value)
        {
            snapshot = value;
            Invalidate();
        }

        public void ShowNear(Point cursor)
        {
            Rectangle area = Screen.FromPoint(cursor).WorkingArea;
            int x = cursor.X - Width + 28;
            int y = cursor.Y - Height - 18;

            if (x < area.Left)
            {
                x = area.Left + 8;
            }
            if (y < area.Top)
            {
                y = cursor.Y + 18;
            }
            if (x + Width > area.Right)
            {
                x = area.Right - Width - 8;
            }
            if (y + Height > area.Bottom)
            {
                y = area.Bottom - Height - 8;
            }

            Location = new Point(x, y);
            Show();
            Activate();
        }

        protected override void OnDeactivate(EventArgs e)
        {
            base.OnDeactivate(e);
            Hide();
        }

        protected override void OnMouseDown(MouseEventArgs e)
        {
            base.OnMouseDown(e);
            if (refreshBounds.Contains(e.Location))
            {
                if (RefreshRequested != null)
                {
                    RefreshRequested(this, EventArgs.Empty);
                }
            }
            else if (settingsBounds.Contains(e.Location))
            {
                if (SettingsRequested != null)
                {
                    SettingsRequested(this, EventArgs.Empty);
                }
            }
        }

        protected override void OnMouseMove(MouseEventArgs e)
        {
            base.OnMouseMove(e);
            bool refreshIsHover = refreshBounds.Contains(e.Location);
            bool settingsIsHover = settingsBounds.Contains(e.Location);
            Cursor = refreshIsHover || settingsIsHover ? Cursors.Hand : Cursors.Default;
            if (refreshHover != refreshIsHover)
            {
                refreshHover = refreshIsHover;
                Invalidate(refreshBounds);
            }
            if (settingsHover != settingsIsHover)
            {
                settingsHover = settingsIsHover;
                Invalidate(settingsBounds);
            }
        }

        protected override void OnMouseLeave(EventArgs e)
        {
            base.OnMouseLeave(e);
            Cursor = Cursors.Default;
            if (refreshHover)
            {
                refreshHover = false;
                Invalidate(refreshBounds);
            }
            if (settingsHover)
            {
                settingsHover = false;
                Invalidate(settingsBounds);
            }
        }

        protected override void OnResize(EventArgs e)
        {
            base.OnResize(e);
            UpdateRegion();
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            base.OnPaint(e);

            Graphics g = e.Graphics;
            g.SmoothingMode = SmoothingMode.AntiAlias;
            g.CompositingQuality = CompositingQuality.HighQuality;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
            g.TextRenderingHint = System.Drawing.Text.TextRenderingHint.ClearTypeGridFit;
            bool dark = IsDarkTheme();
            Color backColor = PopupBackColor(dark);
            Color foreColor = dark ? Color.White : Color.Black;
            Color mutedColor = dark ? Color.Gainsboro : Color.FromArgb(72, 72, 72);
            Color borderColor = dark ? Color.FromArgb(72, 72, 72) : Color.FromArgb(185, 185, 185);
            if (BackColor != backColor)
            {
                BackColor = backColor;
            }
            ForeColor = foreColor;
            g.Clear(backColor);

            using (Font titleFont = new Font("Segoe UI", 10, FontStyle.Regular))
            using (Font labelFont = new Font("Segoe UI", 9, FontStyle.Regular))
            using (Font smallFont = new Font("Segoe UI", 8, FontStyle.Regular))
            using (Brush textBrush = new SolidBrush(foreColor))
            using (Brush mutedBrush = new SolidBrush(mutedColor))
            using (Pen borderPen = new Pen(borderColor))
            {
                g.DrawRectangle(borderPen, 0, 0, Width - 1, Height - 1);
                g.DrawString("Codex Usage", titleFont, textBrush, 14, 11);
                DrawActionGlyphs(g, mutedBrush);

                if (snapshot == null)
                {
                    g.DrawString("Checking limits...", labelFont, mutedBrush, 14, 46);
                    return;
                }

                if (!string.IsNullOrEmpty(snapshot.ErrorMessage))
                {
                    g.DrawString(snapshot.ErrorMessage, labelFont, mutedBrush, new RectangleF(14, 44, Width - 28, 58));
                    g.DrawString("Open settings from the gear.", smallFont, mutedBrush, 14, 120);
                    return;
                }

                DrawUsageRow(g, "Weekly", snapshot.Weekly, snapshot.LastUpdated, 10, 42);
                DrawUsageRow(g, "5h Window", snapshot.FiveHour, snapshot.LastUpdated, 10, 110);
            }
        }

        private void DrawUsageRow(Graphics g, string label, LimitWindow window, DateTime lastUpdated, int x, int y)
        {
            using (Font labelFont = new Font("Segoe UI", 9, FontStyle.Regular))
            using (Font percentFont = new Font("Segoe UI", 8, FontStyle.Bold))
            using (Font smallFont = new Font("Segoe UI", 8, FontStyle.Regular))
            {
                bool dark = IsDarkTheme();
                using (Brush textBrush = new SolidBrush(dark ? Color.White : Color.Black))
                using (Brush mutedBrush = new SolidBrush(dark ? Color.Gainsboro : Color.FromArgb(72, 72, 72)))
                using (Brush trackBrush = new SolidBrush(dark ? Color.FromArgb(22, 22, 22) : Color.FromArgb(210, 210, 210)))
                {
                int remaining = window != null ? window.RemainingPercent : 0;

                int textX = x + 14;
                g.DrawString(label, labelFont, textBrush, textX, y);

                Rectangle bar = new Rectangle(textX, y + 21, Width - textX - 14, 20);
                g.FillRectangle(trackBrush, bar);

                if (window == null)
                {
                    g.DrawString("unknown", smallFont, mutedBrush, bar.X + 6, bar.Y + 3);
                    return;
                }

                Rectangle fill = new Rectangle(bar.X, bar.Y, (int)Math.Round(bar.Width * (remaining / 100.0)), bar.Height);
                using (Brush fillBrush = new SolidBrush(settings.ColorBars ? IconRenderer.ColorForPercent(remaining, settings) : Color.FromArgb(118, 118, 118)))
                {
                    g.FillRectangle(fillBrush, fill);
                }

                DrawCenteredText(g, remaining + "%", percentFont, textBrush, bar);

                string leftDetail = settings == null || settings.ShowPopupLastUpdated
                    ? "Updated: " + TimeFormatter.FormatClock(lastUpdated)
                    : "";
                string reset = window.ResetAfterSeconds.HasValue
                    ? "Next reset: " + TimeFormatter.FormatResetDateTime(lastUpdated, window.ResetAfterSeconds.Value) +
                        " (" + TimeFormatter.FormatDuration(window.ResetAfterSeconds.Value) + ")"
                    : "Next reset: ?";
                string rightDetail = settings == null || settings.ShowPopupResetTimes ? reset : "";

                if (!string.IsNullOrEmpty(leftDetail))
                {
                    g.DrawString(leftDetail, smallFont, mutedBrush, textX, y + 44);
                }
                if (!string.IsNullOrEmpty(rightDetail))
                {
                    DrawRightAlignedText(g, rightDetail, smallFont, mutedBrush, new RectangleF(textX + 106, y + 44, bar.Right - textX - 106, 16));
                }
                }
            }
        }

        private void DrawActionGlyphs(Graphics g, Brush brush)
        {
            refreshBounds = new Rectangle(332, 9, 22, 22);
            settingsBounds = new Rectangle(360, 9, 22, 22);
            bool refreshIsHover = refreshHover || refreshBounds.Contains(PointToClient(Cursor.Position));
            bool settingsIsHover = settingsHover || settingsBounds.Contains(PointToClient(Cursor.Position));
            bool dark = IsDarkTheme();

            if (refreshIsHover)
            {
                using (Brush hoverBrush = new SolidBrush(dark ? Color.FromArgb(92, 92, 92) : Color.FromArgb(220, 220, 220)))
                {
                    g.FillEllipse(hoverBrush, refreshBounds);
                }
            }
            if (settingsIsHover)
            {
                using (Brush hoverBrush = new SolidBrush(dark ? Color.FromArgb(92, 92, 92) : Color.FromArgb(220, 220, 220)))
                {
                    g.FillEllipse(hoverBrush, settingsBounds);
                }
            }

            Color refreshColor = dark
                ? (refreshIsHover ? Color.White : Color.FromArgb(220, 220, 220))
                : (refreshIsHover ? Color.Black : Color.FromArgb(72, 72, 72));
            Color gearColor = dark
                ? (settingsIsHover ? Color.White : Color.FromArgb(220, 220, 220))
                : (settingsIsHover ? Color.Black : Color.FromArgb(72, 72, 72));
            using (Pen refreshPen = new Pen(refreshColor, 1.4f))
            using (Pen gearPen = new Pen(gearColor, 1.2f))
            {
                DrawRefreshIcon(g, refreshPen, refreshBounds);
                DrawSettingsIcon(g, gearPen, settingsBounds);
            }
        }

        private void ApplyThemeColors()
        {
            bool dark = IsDarkTheme();
            BackColor = PopupBackColor(dark);
            ForeColor = dark ? Color.White : Color.Black;
        }

        private bool IsDarkTheme()
        {
            return settings == null || AppSettings.IsDarkTheme(settings.Theme);
        }

        private static Color PopupBackColor(bool dark)
        {
            return dark ? Color.FromArgb(54, 54, 54) : Color.FromArgb(245, 245, 245);
        }

        private static void DrawSettingsIcon(Graphics g, Pen pen, Rectangle bounds)
        {
            float centerX = bounds.Left + (bounds.Width / 2.0f);
            float centerY = bounds.Top + (bounds.Height / 2.0f);
            using (GraphicsPath gearPath = CreateGearPath(centerX, centerY, 7.3f, 5.6f, 8))
            {
                g.DrawPath(pen, gearPath);
            }

            g.DrawEllipse(pen, centerX - 2.2f, centerY - 2.2f, 4.4f, 4.4f);
        }

        private static void DrawRefreshIcon(Graphics g, Pen pen, Rectangle bounds)
        {
            RectangleF arcBounds = new RectangleF(bounds.Left + 5.0f, bounds.Top + 5.0f, bounds.Width - 10.0f, bounds.Height - 10.0f);
            g.DrawArc(pen, arcBounds, 35, 285);

            PointF tip = new PointF(bounds.Right - 5.0f, bounds.Top + 9.0f);
            PointF wingA = new PointF(tip.X - 4.2f, tip.Y - 0.8f);
            PointF wingB = new PointF(tip.X - 1.4f, tip.Y + 3.8f);
            using (Brush brush = new SolidBrush(pen.Color))
            {
                g.FillPolygon(brush, new PointF[] { tip, wingA, wingB });
            }
        }

        private static GraphicsPath CreateGearPath(float centerX, float centerY, float outerRadius, float rootRadius, int teeth)
        {
            GraphicsPath path = new GraphicsPath();
            List<PointF> points = new List<PointF>();
            double step = (Math.PI * 2.0) / teeth;
            for (int tooth = 0; tooth < teeth; tooth++)
            {
                double centerAngle = (-Math.PI / 2.0) + (tooth * step);
                points.Add(CreateGearPoint(centerX, centerY, rootRadius, centerAngle - (step * 0.42)));
                points.Add(CreateGearPoint(centerX, centerY, outerRadius, centerAngle - (step * 0.24)));
                points.Add(CreateGearPoint(centerX, centerY, outerRadius, centerAngle + (step * 0.24)));
                points.Add(CreateGearPoint(centerX, centerY, rootRadius, centerAngle + (step * 0.42)));
            }
            if (points.Count > 0)
            {
                path.AddPolygon(points.ToArray());
            }
            return path;
        }

        private static PointF CreateGearPoint(float centerX, float centerY, float radius, double angle)
        {
            return new PointF(
                centerX + (float)(Math.Cos(angle) * radius),
                centerY + (float)(Math.Sin(angle) * radius));
        }


        private static void DrawCenteredText(Graphics g, string text, Font font, Brush brush, Rectangle bounds)
        {
            using (StringFormat format = new StringFormat())
            {
                format.Alignment = StringAlignment.Center;
                format.LineAlignment = StringAlignment.Center;
                g.DrawString(text, font, brush, bounds, format);
            }
        }

        private static void DrawRightAlignedText(Graphics g, string text, Font font, Brush brush, RectangleF bounds)
        {
            using (StringFormat format = new StringFormat())
            {
                format.Alignment = StringAlignment.Far;
                format.LineAlignment = StringAlignment.Near;
                g.DrawString(text, font, brush, bounds, format);
            }
        }

        private void UpdateRegion()
        {
            if (Width <= 0 || Height <= 0)
            {
                return;
            }

            Region oldRegion = Region;
            using (GraphicsPath path = RoundedRectangle(new Rectangle(0, 0, Width, Height), 8))
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
            int diameter = radius * 2;
            GraphicsPath path = new GraphicsPath();
            path.AddArc(bounds.Left, bounds.Top, diameter, diameter, 180, 90);
            path.AddArc(bounds.Right - diameter - 1, bounds.Top, diameter, diameter, 270, 90);
            path.AddArc(bounds.Right - diameter - 1, bounds.Bottom - diameter - 1, diameter, diameter, 0, 90);
            path.AddArc(bounds.Left, bounds.Bottom - diameter - 1, diameter, diameter, 90, 90);
            path.CloseFigure();
            return path;
        }
    }
}
