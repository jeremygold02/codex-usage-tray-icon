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
    internal static class IconRenderer
    {
        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool DestroyIcon(IntPtr hIcon);

        public static Icon CreatePercentIcon(int percent, AppSettings settings)
        {
            return CreateTextIcon(percent.ToString(), ColorForPercent(percent, settings), Color.White);
        }

        public static Icon CreateUnknownIcon(string text)
        {
            return CreateTextIcon(text, Color.FromArgb(64, 64, 64), Color.White);
        }

        public static Icon CreateErrorIcon()
        {
            return CreateTextIcon("!", Color.FromArgb(210, 46, 46), Color.White);
        }

        public static Color ColorForPercent(int percent, AppSettings settings)
        {
            int critical = settings != null ? settings.CriticalThreshold : 15;
            int low = settings != null ? settings.LowThreshold : 25;

            if (percent <= critical)
            {
                return Color.FromArgb(220, 40, 40);
            }
            if (percent <= low)
            {
                return Color.FromArgb(240, 210, 0);
            }
            return Color.FromArgb(0, 120, 215);
        }

        public static Bitmap CreatePreviewBitmap(int percent, AppSettings settings)
        {
            return CreateTextBitmap(percent.ToString(), ColorForPercent(percent, settings), Color.White, 18);
        }

        private static Icon CreateTextIcon(string text, Color background, Color foreground)
        {
            const int iconWidth = 32;
            const int iconHeight = 32;

            using (Bitmap bitmap = new Bitmap(iconWidth, iconHeight))
            using (Graphics g = Graphics.FromImage(bitmap))
            using (Brush backgroundBrush = new SolidBrush(background))
            using (Brush foregroundBrush = new SolidBrush(foreground))
            {
                g.SmoothingMode = SmoothingMode.AntiAlias;
                g.TextRenderingHint = System.Drawing.Text.TextRenderingHint.ClearTypeGridFit;
                g.Clear(Color.Transparent);
                g.FillRectangle(backgroundBrush, -2, -2, iconWidth + 4, iconHeight + 4);

                if (!string.IsNullOrEmpty(text))
                {
                    RectangleF bounds;
                    if (text == "!")
                    {
                        bounds = new RectangleF(4, 4, 24, 24);
                    }
                    else if (text == "100")
                    {
                        bounds = new RectangleF(3, 6, 26, 18);
                    }
                    else if (text.Length >= 3)
                    {
                        bounds = new RectangleF(1, 3, 30, 25);
                    }
                    else
                    {
                        bounds = new RectangleF(1, 6, 30, 20);
                    }
                    using (FontFamily family = CreateTrayIconFontFamily(text))
                    using (GraphicsPath textPath = CreateTrayIconTextPath(text, family, bounds))
                    {
                        g.FillPath(foregroundBrush, textPath);
                    }
                }

                IntPtr handle = bitmap.GetHicon();
                try
                {
                    using (Icon tempIcon = Icon.FromHandle(handle))
                    {
                        return (Icon)tempIcon.Clone();
                    }
                }
                finally
                {
                    DestroyIcon(handle);
                }
            }
        }

        private static GraphicsPath CreateRoundedRectanglePath(RectangleF bounds, float radius)
        {
            GraphicsPath path = new GraphicsPath();
            if (radius <= 0)
            {
                path.AddRectangle(bounds);
                path.CloseFigure();
                return path;
            }

            float diameter = radius * 2.0f;
            path.AddArc(bounds.X, bounds.Y, diameter, diameter, 180, 90);
            path.AddArc(bounds.Right - diameter, bounds.Y, diameter, diameter, 270, 90);
            path.AddArc(bounds.Right - diameter, bounds.Bottom - diameter, diameter, diameter, 0, 90);
            path.AddArc(bounds.X, bounds.Bottom - diameter, diameter, diameter, 90, 90);
            path.CloseFigure();
            return path;
        }

        private static FontFamily CreateTrayIconFontFamily(string text)
        {
            string[] candidates = text == "!"
                ? new string[] { "Segoe UI", "Microsoft Sans Serif" }
                : text.Length >= 3
                    ? new string[] { "Segoe UI Semibold", "Arial Narrow", "Segoe UI", "Microsoft Sans Serif" }
                    : new string[] { "Segoe UI Semibold", "Segoe UI", "Microsoft Sans Serif" };
            for (int i = 0; i < candidates.Length; i++)
            {
                try
                {
                    FontFamily family = new FontFamily(candidates[i]);
                    if (string.Equals(family.Name, candidates[i], StringComparison.OrdinalIgnoreCase))
                    {
                        return family;
                    }
                    family.Dispose();
                }
                catch
                {
                }
            }

            return new FontFamily("Microsoft Sans Serif");
        }

        private static GraphicsPath CreateTrayIconTextPath(string text, FontFamily family, RectangleF bounds)
        {
            GraphicsPath path = new GraphicsPath();
            using (StringFormat format = (StringFormat)StringFormat.GenericTypographic.Clone())
            {
                format.FormatFlags = StringFormatFlags.NoWrap;
                path.AddString(text, family, (int)FontStyle.Regular, 100.0f, new PointF(0, 0), format);
            }

            RectangleF originalBounds = path.GetBounds();
            float scale = Math.Min(bounds.Width / originalBounds.Width, bounds.Height / originalBounds.Height);
            using (Matrix normalize = new Matrix())
            {
                normalize.Translate(-originalBounds.X, -originalBounds.Y);
                path.Transform(normalize);
            }
            using (Matrix scaleMatrix = new Matrix())
            {
                scaleMatrix.Scale(scale, scale);
                path.Transform(scaleMatrix);
            }

            RectangleF scaledBounds = path.GetBounds();
            float x = bounds.X + ((bounds.Width - scaledBounds.Width) / 2.0f) - scaledBounds.X;
            float y = bounds.Y + ((bounds.Height - scaledBounds.Height) / 2.0f) - scaledBounds.Y;
            using (Matrix position = new Matrix())
            {
                position.Translate(x, y);
                path.Transform(position);
            }

            if (text.Length == 2)
            {
                RectangleF finalBounds = path.GetBounds();
                float centerX = finalBounds.X + (finalBounds.Width / 2.0f);
                float centerY = finalBounds.Y + (finalBounds.Height / 2.0f);
                using (Matrix squeeze = new Matrix())
                {
                    squeeze.Translate(centerX, centerY);
                    squeeze.Scale(0.92f, 0.92f);
                    squeeze.Translate(-centerX, -centerY);
                    path.Transform(squeeze);
                }
            }

            return path;
        }

        private static Bitmap CreateTextBitmap(string text, Color background, Color foreground, int size)
        {
            Bitmap bitmap = new Bitmap(size, size);
            using (Graphics g = Graphics.FromImage(bitmap))
            using (Brush backgroundBrush = new SolidBrush(background))
            using (Brush foregroundBrush = new SolidBrush(foreground))
            using (Brush shadowBrush = new SolidBrush(Color.FromArgb(130, 0, 0, 0)))
            {
                g.SmoothingMode = SmoothingMode.AntiAlias;
                g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                g.CompositingQuality = CompositingQuality.HighQuality;
                g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                g.TextRenderingHint = System.Drawing.Text.TextRenderingHint.AntiAlias;
                g.FillRectangle(backgroundBrush, 0, 0, size, size);

                if (string.IsNullOrEmpty(text))
                {
                    return bitmap;
                }

                RectangleF bounds = size == 64
                    ? new RectangleF(1, 1, size - 2, size - 2)
                    : new RectangleF(1, 1, size - 2, size - 2);

                using (FontFamily family = CreateTrayIconFontFamily(text))
                using (GraphicsPath textPath = CreateCenteredTextPath(text, family, bounds, size))
                {
                    float shadowOffset = size == 64 ? 1.2f : 0.6f;
                    using (GraphicsPath shadowPath = (GraphicsPath)textPath.Clone())
                    using (Matrix shadowMatrix = new Matrix())
                    {
                        shadowMatrix.Translate(shadowOffset, shadowOffset);
                        shadowPath.Transform(shadowMatrix);
                        g.FillPath(shadowBrush, shadowPath);
                    }
                    g.FillPath(foregroundBrush, textPath);
                }
            }

            return bitmap;
        }

        private static GraphicsPath CreateCenteredTextPath(string text, FontFamily family, RectangleF bounds, int iconSize)
        {
            float fontSize = FitFontSize(text, family, bounds, iconSize);
            GraphicsPath path = CreateTextPath(text, family, fontSize);
            RectangleF textBounds = path.GetBounds();
            float x = bounds.X + ((bounds.Width - textBounds.Width) / 2.0f) - textBounds.X;
            float y = bounds.Y + ((bounds.Height - textBounds.Height) / 2.0f) - textBounds.Y;
            using (Matrix matrix = new Matrix())
            {
                matrix.Translate(x, y);
                path.Transform(matrix);
            }
            return path;
        }

        private static float FitFontSize(string text, FontFamily family, RectangleF bounds, int iconSize)
        {
            float fontSize;
            if (text == "!")
            {
                fontSize = iconSize * 0.78f;
            }
            else if (text.Length >= 3)
            {
                fontSize = iconSize * 0.62f;
            }
            else
            {
                fontSize = iconSize * 0.84f;
            }

            float minimum = Math.Max(8.0f, iconSize * 0.24f);
            while (fontSize > minimum)
            {
                using (GraphicsPath path = CreateTextPath(text, family, fontSize))
                {
                    RectangleF measured = path.GetBounds();
                    if (measured.Width <= bounds.Width && measured.Height <= bounds.Height)
                    {
                        break;
                    }
                }

                fontSize -= 1.0f;
            }

            return fontSize;
        }

        private static GraphicsPath CreateTextPath(string text, FontFamily family, float fontSize)
        {
            GraphicsPath path = new GraphicsPath();
            using (StringFormat format = (StringFormat)StringFormat.GenericTypographic.Clone())
            {
                format.FormatFlags = StringFormatFlags.NoWrap;
                path.AddString(text, family, (int)FontStyle.Regular, fontSize, new PointF(0, 0), format);
            }
            return path;
        }
    }
}
