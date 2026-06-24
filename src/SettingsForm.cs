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
    internal sealed class SettingsForm : Form
    {
        private readonly AppSettings settings;
        private CheckBox colorBarsCheckBox;
        private NumericUpDown criticalNumeric;
        private NumericUpDown lowNumeric;
        private NumericUpDown refreshNumeric;
        private NumericUpDown idleRefreshNumeric;
        private RadioButton weeklyRadio;
        private RadioButton fiveHourRadio;
        private ComboBox themeCombo;
        private CheckBox showResetTimesCheckBox;
        private CheckBox showLastUpdatedCheckBox;
        private CheckBox startWithWindowsCheckBox;
        private CheckBox thresholdNotificationsCheckBox;
        private CheckBox showTrayBoxCheckBox;
        private PictureBox criticalPreview;
        private PictureBox lowPreview;
        private PictureBox normalPreview;
        private Button trayBoxColorButton;
        private Button trayBoxAutoButton;
        private Button trayTextColorButton;
        private Button checkUpdatesButton;
        private Label versionLabel;
        private Label updateStatusLabel;
        private bool useCustomTrayBoxColor;
        private Color trayBoxColor;
        private Color trayTextColor;

        public event EventHandler SettingsApplied;
        public event EventHandler CheckUpdatesRequested;

        public SettingsForm(AppSettings settings)
        {
            this.settings = settings;
            Text = "Codex Usage Tray Settings";
            ClientSize = new Size(384, 556);
            FormBorderStyle = FormBorderStyle.FixedDialog;
            MaximizeBox = false;
            MinimizeBox = false;
            ShowIcon = true;
            ShowInTaskbar = true;
            StartPosition = FormStartPosition.CenterScreen;
            BackColor = Color.FromArgb(54, 54, 54);
            ForeColor = Color.White;
            Font = new Font("Segoe UI", 9, FontStyle.Regular);
            AutoScaleMode = AutoScaleMode.None;
            SetWindowIcon();
            BuildUi();
            LoadSettings();
            ApplyTheme(this, AppSettings.IsDarkTheme(GetSelectedTheme()));
            UpdateColorButtons();
        }

        private void SetWindowIcon()
        {
            try
            {
                Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath);
            }
            catch
            {
            }
        }

        private void BuildUi()
        {
            GroupBox notificationGroup = CreateGroup("Notification / Tray Icon", 14, 14, 356, 218);
            notificationGroup.Controls.Add(CreateLabel("Warning thresholds", 20, 24, 160));

            criticalPreview = CreatePreviewBox(20, 50);
            notificationGroup.Controls.Add(criticalPreview);
            notificationGroup.Controls.Add(CreateCaptionLabel("Critical Threshold", 68, 45, 104));
            criticalNumeric = CreateNumeric(68, 67, 58, 1, 99);
            notificationGroup.Controls.Add(criticalNumeric);
            notificationGroup.Controls.Add(CreateLabel("%", 130, 70, 18));

            lowPreview = CreatePreviewBox(174, 50);
            notificationGroup.Controls.Add(lowPreview);
            notificationGroup.Controls.Add(CreateCaptionLabel("Low Threshold", 220, 45, 88));
            lowNumeric = CreateNumeric(220, 67, 58, 1, 99);
            notificationGroup.Controls.Add(lowNumeric);
            notificationGroup.Controls.Add(CreateLabel("%", 282, 70, 18));

            normalPreview = CreatePreviewBox(312, 50);
            notificationGroup.Controls.Add(normalPreview);

            notificationGroup.Controls.Add(CreateLabel("Tray icon reflects the:", 20, 104, 220));
            weeklyRadio = CreateRadio("Weekly usage remaining", 28, 128, 260);
            fiveHourRadio = CreateRadio("5h usage remaining", 28, 154, 260);
            notificationGroup.Controls.Add(weeklyRadio);
            notificationGroup.Controls.Add(fiveHourRadio);

            notificationGroup.Controls.Add(CreateLabel("Active", 20, 186, 44));
            refreshNumeric = CreateNumeric(68, 183, 56, 30, 3600);
            notificationGroup.Controls.Add(refreshNumeric);
            notificationGroup.Controls.Add(CreateLabel("sec", 130, 186, 28));
            notificationGroup.Controls.Add(CreateLabel("Idle (0=off)", 164, 186, 76));
            idleRefreshNumeric = CreateNumeric(240, 183, 56, 0, 7200);
            notificationGroup.Controls.Add(idleRefreshNumeric);
            notificationGroup.Controls.Add(CreateLabel("sec", 302, 186, 28));
            thresholdNotificationsCheckBox = CreateCheckBox("Alerts", 292, 154, 58);
            notificationGroup.Controls.Add(thresholdNotificationsCheckBox);
            Controls.Add(notificationGroup);

            GroupBox colorGroup = CreateGroup("Color Settings", 14, 246, 356, 128);
            themeCombo = new ComboBox();
            themeCombo.DropDownStyle = ComboBoxStyle.DropDownList;
            themeCombo.FlatStyle = FlatStyle.Flat;
            themeCombo.Items.AddRange(new object[] { AppSettings.ThemeSystem, AppSettings.ThemeDark, AppSettings.ThemeLight });
            themeCombo.Location = new Point(22, 28);
            themeCombo.Size = new Size(312, 24);
            colorGroup.Controls.Add(themeCombo);
            colorBarsCheckBox = CreateCheckBox("Color usage level bar", 22, 58, 170);
            colorGroup.Controls.Add(colorBarsCheckBox);
            showTrayBoxCheckBox = CreateCheckBox("Show tray box", 212, 58, 126);
            showTrayBoxCheckBox.CheckedChanged += delegate { UpdatePreviews(); };
            colorGroup.Controls.Add(showTrayBoxCheckBox);
            colorGroup.Controls.Add(CreateLabel("Box", 22, 90, 30));
            trayBoxColorButton = CreateColorButton(58, 86);
            trayBoxColorButton.Click += delegate { PickTrayBoxColor(); };
            colorGroup.Controls.Add(trayBoxColorButton);
            trayBoxAutoButton = CreateButton("Auto", 106, 84, 52);
            trayBoxAutoButton.Click += delegate
            {
                useCustomTrayBoxColor = false;
                UpdateColorButtons();
                UpdatePreviews();
            };
            colorGroup.Controls.Add(trayBoxAutoButton);
            colorGroup.Controls.Add(CreateLabel("Text", 182, 90, 36));
            trayTextColorButton = CreateColorButton(220, 86);
            trayTextColorButton.Click += delegate { PickTrayTextColor(); };
            colorGroup.Controls.Add(trayTextColorButton);
            Controls.Add(colorGroup);

            GroupBox helpGroup = CreateGroup("Popup / Menu", 14, 388, 356, 92);
            showLastUpdatedCheckBox = CreateCheckBox("Show last-updated times", 22, 28, 174);
            helpGroup.Controls.Add(showLastUpdatedCheckBox);
            showResetTimesCheckBox = CreateCheckBox("Show reset times", 202, 28, 132);
            helpGroup.Controls.Add(showResetTimesCheckBox);
            startWithWindowsCheckBox = CreateCheckBox("Start with Windows", 22, 54, 174);
            helpGroup.Controls.Add(startWithWindowsCheckBox);
            Controls.Add(helpGroup);

            versionLabel = CreateLabel("Version " + AppVersion.Current, 22, 486, 110);
            Controls.Add(versionLabel);

            updateStatusLabel = CreateLabel("", 136, 486, 232);
            updateStatusLabel.TextAlign = ContentAlignment.TopRight;
            Controls.Add(updateStatusLabel);

            Button defaultsButton = CreateButton("Defaults", 22, 512, 78);
            defaultsButton.Click += delegate { RestoreDefaults(); };
            Controls.Add(defaultsButton);

            checkUpdatesButton = CreateButton("Check Updates", 108, 512, 100);
            checkUpdatesButton.Click += delegate
            {
                if (CheckUpdatesRequested != null)
                {
                    CheckUpdatesRequested(this, EventArgs.Empty);
                }
            };
            Controls.Add(checkUpdatesButton);

            Button okButton = CreateButton("OK", 216, 512, 76);
            okButton.Click += delegate
            {
                ApplyToSettings();
                Close();
            };
            Controls.Add(okButton);

            Button cancelButton = CreateButton("Cancel", 300, 512, 76);
            cancelButton.Click += delegate { Close(); };
            Controls.Add(cancelButton);
        }

        private void LoadSettings()
        {
            criticalNumeric.Value = settings.CriticalThreshold;
            lowNumeric.Value = settings.LowThreshold;
            weeklyRadio.Checked = !string.Equals(settings.IconMetric, AppSettings.IconMetricFiveHour, StringComparison.OrdinalIgnoreCase);
            fiveHourRadio.Checked = string.Equals(settings.IconMetric, AppSettings.IconMetricFiveHour, StringComparison.OrdinalIgnoreCase);
            colorBarsCheckBox.Checked = settings.ColorBars;
            showResetTimesCheckBox.Checked = settings.ShowPopupResetTimes;
            showLastUpdatedCheckBox.Checked = settings.ShowPopupLastUpdated;
            startWithWindowsCheckBox.Checked = StartupManager.IsEnabled();
            thresholdNotificationsCheckBox.Checked = settings.ThresholdNotifications;
            showTrayBoxCheckBox.Checked = settings.ShowTrayBox;
            useCustomTrayBoxColor = settings.UseCustomTrayBoxColor;
            trayBoxColor = settings.GetTrayBoxColor();
            trayTextColor = settings.GetTrayTextColor();
            refreshNumeric.Value = Math.Max(30, settings.RefreshSeconds);
            idleRefreshNumeric.Value = Math.Max(0, settings.IdleRefreshSeconds);
            SelectTheme(settings.Theme);
            UpdatePreviews();

            criticalNumeric.ValueChanged += delegate { UpdatePreviews(); };
            lowNumeric.ValueChanged += delegate { UpdatePreviews(); };
            themeCombo.SelectedIndexChanged += delegate
            {
                ApplyTheme(this, AppSettings.IsDarkTheme(GetSelectedTheme()));
                UpdateColorButtons();
            };
        }

        private void ApplyToSettings()
        {
            settings.CriticalThreshold = (int)criticalNumeric.Value;
            settings.LowThreshold = (int)lowNumeric.Value;
            if (settings.CriticalThreshold > settings.LowThreshold)
            {
                settings.LowThreshold = settings.CriticalThreshold;
                lowNumeric.Value = settings.LowThreshold;
            }
            settings.IconMetric = fiveHourRadio.Checked ? AppSettings.IconMetricFiveHour : AppSettings.IconMetricWeekly;
            settings.ColorBars = colorBarsCheckBox.Checked;
            settings.ShowPopupResetTimes = showResetTimesCheckBox.Checked;
            settings.ShowPopupLastUpdated = showLastUpdatedCheckBox.Checked;
            settings.StartWithWindows = startWithWindowsCheckBox.Checked;
            settings.ThresholdNotifications = thresholdNotificationsCheckBox.Checked;
            settings.ShowTrayBox = showTrayBoxCheckBox.Checked;
            settings.UseCustomTrayBoxColor = useCustomTrayBoxColor;
            settings.TrayBoxColor = AppSettings.FormatColor(trayBoxColor);
            settings.TrayTextColor = AppSettings.FormatColor(trayTextColor);
            settings.RefreshSeconds = (int)refreshNumeric.Value;
            settings.IdleRefreshSeconds = (int)idleRefreshNumeric.Value;
            if (settings.IdleRefreshSeconds > 0 && settings.IdleRefreshSeconds < settings.RefreshSeconds)
            {
                settings.IdleRefreshSeconds = settings.RefreshSeconds;
                idleRefreshNumeric.Value = settings.IdleRefreshSeconds;
            }
            settings.Theme = GetSelectedTheme();

            if (SettingsApplied != null)
            {
                SettingsApplied(this, EventArgs.Empty);
            }
        }

        private void RestoreDefaults()
        {
            AppSettings defaults = new AppSettings();
            criticalNumeric.Value = defaults.CriticalThreshold;
            lowNumeric.Value = defaults.LowThreshold;
            weeklyRadio.Checked = true;
            colorBarsCheckBox.Checked = defaults.ColorBars;
            showResetTimesCheckBox.Checked = defaults.ShowPopupResetTimes;
            showLastUpdatedCheckBox.Checked = defaults.ShowPopupLastUpdated;
            startWithWindowsCheckBox.Checked = defaults.StartWithWindows;
            thresholdNotificationsCheckBox.Checked = defaults.ThresholdNotifications;
            showTrayBoxCheckBox.Checked = defaults.ShowTrayBox;
            useCustomTrayBoxColor = defaults.UseCustomTrayBoxColor;
            trayBoxColor = defaults.GetTrayBoxColor();
            trayTextColor = defaults.GetTrayTextColor();
            refreshNumeric.Value = defaults.RefreshSeconds;
            idleRefreshNumeric.Value = defaults.IdleRefreshSeconds;
            SelectTheme(defaults.Theme);
            ApplyTheme(this, AppSettings.IsDarkTheme(GetSelectedTheme()));
            UpdateColorButtons();
            UpdatePreviews();
        }

        public void SetUpdateButtonState(bool busy, string busyText)
        {
            if (checkUpdatesButton == null)
            {
                return;
            }

            checkUpdatesButton.Enabled = !busy;
            checkUpdatesButton.Text = busy ? busyText : "Check Updates";
        }

        public void SetUpdateStatus(string status)
        {
            if (updateStatusLabel == null)
            {
                return;
            }

            updateStatusLabel.Text = status ?? "";
        }

        private void PickTrayBoxColor()
        {
            Color selected;
            if (TryPickColor(trayBoxColor, out selected))
            {
                trayBoxColor = selected;
                useCustomTrayBoxColor = true;
                UpdateColorButtons();
                UpdatePreviews();
            }
        }

        private void PickTrayTextColor()
        {
            Color selected;
            if (TryPickColor(trayTextColor, out selected))
            {
                trayTextColor = selected;
                UpdateColorButtons();
                UpdatePreviews();
            }
        }

        private bool TryPickColor(Color currentColor, out Color selectedColor)
        {
            using (ColorDialog dialog = new ColorDialog())
            {
                dialog.FullOpen = true;
                dialog.Color = currentColor;
                if (dialog.ShowDialog(this) == DialogResult.OK)
                {
                    selectedColor = dialog.Color;
                    return true;
                }
            }

            selectedColor = currentColor;
            return false;
        }

        private void UpdateColorButtons()
        {
            if (trayBoxColorButton != null)
            {
                trayBoxColorButton.BackColor = trayBoxColor;
                trayBoxColorButton.ForeColor = ContrastColor(trayBoxColor);
                trayBoxColorButton.Text = useCustomTrayBoxColor ? "" : "A";
            }
            if (trayTextColorButton != null)
            {
                trayTextColorButton.BackColor = trayTextColor;
                trayTextColorButton.ForeColor = ContrastColor(trayTextColor);
                trayTextColorButton.Text = "";
            }
            if (trayBoxAutoButton != null)
            {
                trayBoxAutoButton.Enabled = useCustomTrayBoxColor;
            }
        }

        private static Color ContrastColor(Color color)
        {
            int brightness = ((color.R * 299) + (color.G * 587) + (color.B * 114)) / 1000;
            return brightness > 140 ? Color.Black : Color.White;
        }

        private void UpdatePreviews()
        {
            AppSettings previewSettings = new AppSettings();
            previewSettings.CriticalThreshold = (int)criticalNumeric.Value;
            previewSettings.LowThreshold = (int)lowNumeric.Value;
            previewSettings.ShowTrayBox = showTrayBoxCheckBox == null || showTrayBoxCheckBox.Checked;
            previewSettings.UseCustomTrayBoxColor = useCustomTrayBoxColor;
            previewSettings.TrayBoxColor = AppSettings.FormatColor(trayBoxColor);
            previewSettings.TrayTextColor = AppSettings.FormatColor(trayTextColor);

            ReplacePreview(criticalPreview, IconRenderer.CreatePreviewBitmap((int)criticalNumeric.Value, previewSettings));
            ReplacePreview(lowPreview, IconRenderer.CreatePreviewBitmap((int)Math.Max(1, lowNumeric.Value), previewSettings));
            ReplacePreview(normalPreview, IconRenderer.CreatePreviewBitmap(100, previewSettings));
        }

        private static void ReplacePreview(PictureBox box, Bitmap bitmap)
        {
            Image old = box.Image;
            box.Image = bitmap;
            if (old != null)
            {
                old.Dispose();
            }
        }

        private static GroupBox CreateGroup(string text, int x, int y, int width, int height)
        {
            GroupBox group = new DarkGroupBox();
            group.Text = text;
            group.Location = new Point(x, y);
            group.Size = new Size(width, height);
            return group;
        }

        private static Label CreateLabel(string text, int x, int y, int width)
        {
            Label label = new Label();
            label.Text = text;
            label.Location = new Point(x, y);
            label.Size = new Size(width, 22);
            return label;
        }

        private static Label CreateCaptionLabel(string text, int x, int y, int width)
        {
            Label label = new CaptionLabel();
            label.Text = text;
            label.Location = new Point(x, y);
            label.Font = new Font("Segoe UI", 8, FontStyle.Regular);
            label.Size = new Size(width, 22);
            label.TextAlign = ContentAlignment.TopLeft;
            label.Margin = Padding.Empty;
            label.Padding = Padding.Empty;
            return label;
        }

        private static CheckBox CreateCheckBox(string text, int x, int y, int width)
        {
            CheckBox checkBox = new CheckBox();
            checkBox.Text = text;
            checkBox.Location = new Point(x, y);
            checkBox.Size = new Size(width, 24);
            return checkBox;
        }

        private static RadioButton CreateRadio(string text, int x, int y, int width)
        {
            RadioButton radio = new RadioButton();
            radio.Text = text;
            radio.Location = new Point(x, y);
            radio.Size = new Size(width, 24);
            return radio;
        }

        private static NumericUpDown CreateNumeric(int x, int y, int width, int min, int max)
        {
            NumericUpDown numeric = new NumericUpDown();
            numeric.Minimum = min;
            numeric.Maximum = max;
            numeric.Location = new Point(x, y);
            numeric.Size = new Size(width, 22);
            return numeric;
        }

        private static PictureBox CreatePreviewBox(int x, int y)
        {
            PictureBox box = new PictureBox();
            box.Location = new Point(x, y);
            box.Size = new Size(38, 38);
            box.SizeMode = PictureBoxSizeMode.CenterImage;
            box.BackColor = Color.FromArgb(104, 104, 104);
            return box;
        }

        private static Button CreateButton(string text, int x, int y, int width)
        {
            Button button = new Button();
            button.Text = text;
            button.Location = new Point(x, y);
            button.Size = new Size(width, 26);
            button.FlatStyle = FlatStyle.Flat;
            button.BackColor = Color.FromArgb(74, 74, 74);
            button.ForeColor = Color.White;
            button.FlatAppearance.BorderColor = Color.FromArgb(150, 150, 150);
            button.FlatAppearance.MouseOverBackColor = Color.FromArgb(92, 92, 92);
            button.FlatAppearance.MouseDownBackColor = Color.FromArgb(58, 58, 58);
            return button;
        }

        private static Button CreateColorButton(int x, int y)
        {
            Button button = new Button();
            button.Location = new Point(x, y);
            button.Size = new Size(42, 24);
            button.FlatStyle = FlatStyle.Flat;
            button.FlatAppearance.BorderColor = Color.FromArgb(150, 150, 150);
            button.FlatAppearance.MouseOverBackColor = Color.FromArgb(92, 92, 92);
            button.FlatAppearance.MouseDownBackColor = Color.FromArgb(58, 58, 58);
            return button;
        }

        private string GetSelectedTheme()
        {
            object selected = themeCombo.SelectedItem;
            return selected != null ? selected.ToString() : AppSettings.ThemeSystem;
        }

        private void SelectTheme(string theme)
        {
            string normalized = string.IsNullOrEmpty(theme) ? AppSettings.ThemeSystem : theme;
            int index = themeCombo.Items.IndexOf(normalized);
            themeCombo.SelectedIndex = index >= 0 ? index : 0;
        }

        private static void ApplyTheme(Control control, bool dark)
        {
            Color backColor = dark ? Color.FromArgb(54, 54, 54) : SystemColors.Control;
            Color foreColor = dark ? Color.White : Color.Black;
            Color inputBackColor = dark ? Color.FromArgb(74, 74, 74) : Color.White;
            Color inputForeColor = dark ? Color.White : Color.Black;
            Color borderColor = dark ? Color.FromArgb(74, 74, 74) : Color.FromArgb(180, 180, 180);

            if (!(control is PictureBox))
            {
                control.BackColor = backColor;
            }
            control.ForeColor = foreColor;

            DarkGroupBox group = control as DarkGroupBox;
            if (group != null)
            {
                group.BorderColor = borderColor;
                group.Invalidate();
            }

            Button button = control as Button;
            if (button != null)
            {
                button.BackColor = dark ? Color.FromArgb(74, 74, 74) : Color.FromArgb(245, 245, 245);
                button.ForeColor = foreColor;
                button.FlatAppearance.BorderColor = dark ? Color.FromArgb(150, 150, 150) : Color.FromArgb(170, 170, 170);
                button.FlatAppearance.MouseOverBackColor = dark ? Color.FromArgb(92, 92, 92) : Color.FromArgb(229, 229, 229);
                button.FlatAppearance.MouseDownBackColor = dark ? Color.FromArgb(58, 58, 58) : Color.FromArgb(214, 214, 214);
            }

            foreach (Control child in control.Controls)
            {
                ApplyTheme(child, dark);
                if (child is TextBox || child is ComboBox || child is NumericUpDown)
                {
                    child.BackColor = child is NumericUpDown && dark ? Color.Black : inputBackColor;
                    child.ForeColor = inputForeColor;
                }
            }
        }
    }

    internal sealed class DarkGroupBox : GroupBox
    {
        public Color BorderColor { get; set; }

        public DarkGroupBox()
        {
            BorderColor = Color.FromArgb(74, 74, 74);
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.Clear(BackColor);
            e.Graphics.TextRenderingHint = System.Drawing.Text.TextRenderingHint.ClearTypeGridFit;

            SizeF textSize = e.Graphics.MeasureString(Text, Font);
            int textX = 8;
            int borderY = Math.Max(8, (int)(textSize.Height / 2));
            int textRight = textX + (int)Math.Ceiling(textSize.Width) + 8;

            using (Pen borderPen = new Pen(BorderColor))
            using (Brush textBrush = new SolidBrush(ForeColor))
            {
                e.Graphics.DrawLine(borderPen, 1, borderY, Math.Max(1, textX - 3), borderY);
                e.Graphics.DrawLine(borderPen, textRight, borderY, Width - 2, borderY);
                e.Graphics.DrawLine(borderPen, 1, borderY, 1, Height - 2);
                e.Graphics.DrawLine(borderPen, Width - 2, borderY, Width - 2, Height - 2);
                e.Graphics.DrawLine(borderPen, 1, Height - 2, Width - 2, Height - 2);
                e.Graphics.DrawString(Text, Font, textBrush, textX, 0);
            }
        }
    }

    internal sealed class CaptionLabel : Label
    {
        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.Clear(BackColor);
            TextRenderer.DrawText(
                e.Graphics,
                Text,
                Font,
                new Rectangle(0, 1, Width, Height - 1),
                ForeColor,
                TextFormatFlags.Left | TextFormatFlags.Top | TextFormatFlags.SingleLine | TextFormatFlags.NoPrefix | TextFormatFlags.NoClipping);
        }
    }
}
