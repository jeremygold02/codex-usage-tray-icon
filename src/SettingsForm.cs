using System;
using System.Drawing;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal sealed class SettingsForm : Form
    {
        private readonly AppSettings settings;
        private readonly ToolTip toolTip;

        private CheckBox colorBarsCheckBox;
        private NumericUpDown criticalNumeric;
        private NumericUpDown lowNumeric;
        private NumericUpDown refreshNumeric;
        private NumericUpDown idleRefreshNumeric;
        private NumericUpDown previewPercentNumeric;
        private RadioButton weeklyRadio;
        private RadioButton fiveHourRadio;
        private ComboBox themeCombo;
        private CheckBox showResetTimesCheckBox;
        private CheckBox showLastUpdatedCheckBox;
        private CheckBox showResetAvailabilityCheckBox;
        private CheckBox startWithWindowsCheckBox;
        private CheckBox thresholdNotificationsCheckBox;
        private CheckBox showTrayBoxCheckBox;
        private PictureBox trayPreview;
        private Button trayBoxColorButton;
        private Button trayBoxAutoButton;
        private Button trayTextColorButton;
        private Button checkUpdatesButton;
        private Label trayBoxHexLabel;
        private Label trayTextHexLabel;
        private Label contrastStatusLabel;
        private Label updateStatusLabel;
        private Control thresholdControls;
        private Control trayBackgroundControls;
        private bool useCustomTrayBoxColor;
        private Color trayBoxColor;
        private Color trayTextColor;
        private bool adjustingValues;

        public event EventHandler SettingsApplied;
        public event EventHandler CheckUpdatesRequested;

        public SettingsForm(AppSettings settings)
        {
            if (settings == null)
            {
                throw new ArgumentNullException("settings");
            }

            this.settings = settings;
            toolTip = new ToolTip();
            toolTip.AutomaticDelay = 350;
            toolTip.AutoPopDelay = 12000;
            toolTip.ShowAlways = true;

            Text = "Codex Usage Tray Settings";
            ClientSize = new Size(384, 561);
            MinimumSize = new Size(400, 600);
            FormBorderStyle = FormBorderStyle.FixedDialog;
            MaximizeBox = false;
            MinimizeBox = false;
            ShowIcon = true;
            ShowInTaskbar = true;
            StartPosition = FormStartPosition.CenterScreen;
            Font = SystemFonts.MessageBoxFont;
            AutoScaleDimensions = new SizeF(96.0F, 96.0F);
            AutoScaleMode = AutoScaleMode.Dpi;

            SetWindowIcon();
            BuildUi();
            LoadSettings();
            WireEvents();
            ApplyCurrentTheme();
            UpdatePreviews();
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                if (trayPreview != null && trayPreview.Image != null)
                {
                    Image image = trayPreview.Image;
                    trayPreview.Image = null;
                    image.Dispose();
                }
                toolTip.Dispose();
            }

            base.Dispose(disposing);
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
            SuspendLayout();

            TableLayoutPanel mainLayout = new TableLayoutPanel();
            mainLayout.ColumnCount = 1;
            mainLayout.RowCount = 6;
            mainLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));
            mainLayout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            mainLayout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            mainLayout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            mainLayout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            mainLayout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            mainLayout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            mainLayout.Dock = DockStyle.Fill;
            mainLayout.Padding = new Padding(8);
            mainLayout.Margin = Padding.Empty;

            mainLayout.Controls.Add(BuildUsageGroup(), 0, 0);
            mainLayout.Controls.Add(BuildRefreshGroup(), 0, 1);
            mainLayout.Controls.Add(BuildAppearanceGroup(), 0, 2);
            mainLayout.Controls.Add(BuildPopupGroup(), 0, 3);
            mainLayout.Controls.Add(BuildSystemGroup(), 0, 4);

            TableLayoutPanel buttonLayout = new TableLayoutPanel();
            buttonLayout.AutoSize = true;
            buttonLayout.ColumnCount = 2;
            buttonLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));
            buttonLayout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            buttonLayout.Dock = DockStyle.Fill;
            buttonLayout.Margin = Padding.Empty;

            Button defaultsButton = CreateButton("&Defaults", "Restore default settings", 84);
            defaultsButton.Anchor = AnchorStyles.Left;
            defaultsButton.Click += delegate { RestoreDefaults(); };
            toolTip.SetToolTip(defaultsButton, "Restore the controls to their default values. Changes are saved only after OK.");
            buttonLayout.Controls.Add(defaultsButton, 0, 0);

            FlowLayoutPanel actionButtons = CreateFlowLayout();
            actionButtons.FlowDirection = FlowDirection.RightToLeft;
            actionButtons.Anchor = AnchorStyles.Right;

            Button cancelButton = CreateButton("&Cancel", "Cancel settings changes", 82);
            cancelButton.DialogResult = DialogResult.Cancel;
            cancelButton.Click += delegate { Close(); };
            actionButtons.Controls.Add(cancelButton);

            Button okButton = CreateButton("&OK", "Apply settings", 82);
            okButton.DialogResult = DialogResult.OK;
            okButton.Click += delegate
            {
                if (!TryApplyToSettings())
                {
                    DialogResult = DialogResult.None;
                    return;
                }
                Close();
            };
            actionButtons.Controls.Add(okButton);

            buttonLayout.Controls.Add(actionButtons, 1, 0);
            mainLayout.Controls.Add(buttonLayout, 0, 5);

            AcceptButton = okButton;
            CancelButton = cancelButton;
            Controls.Add(mainLayout);
            ResumeLayout(false);
        }

        private GroupBox BuildUsageGroup()
        {
            GroupBox group = CreateGroup("Usage & alerts");
            TableLayoutPanel layout = CreateGroupLayout(1);

            TableLayoutPanel metricRow = CreateGroupLayout(2);
            metricRow.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            metricRow.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));

            Label metricLabel = CreateRowLabel("Tray &icon shows", "Tray icon metric");
            metricLabel.MinimumSize = new Size(92, 0);
            metricRow.Controls.Add(metricLabel, 0, 0);

            FlowLayoutPanel metricOptions = CreateFlowLayout();
            weeklyRadio = CreateRadio("&Weekly", "Weekly usage remaining");
            fiveHourRadio = CreateRadio("&5-hour", "Five-hour usage remaining");
            metricOptions.Controls.Add(weeklyRadio);
            metricOptions.Controls.Add(fiveHourRadio);
            toolTip.SetToolTip(weeklyRadio, "Show weekly usage remaining in the tray icon.");
            toolTip.SetToolTip(
                fiveHourRadio,
                "Show five-hour usage remaining. Weekly usage is used when the five-hour limit is unavailable.");
            metricRow.Controls.Add(metricOptions, 1, 0);
            layout.Controls.Add(metricRow, 0, 0);

            thresholdNotificationsCheckBox = CreateCheckBox(
                "&Usage threshold alerts",
                "Usage threshold alerts");
            thresholdNotificationsCheckBox.Margin = new Padding(3, 3, 3, 1);
            toolTip.SetToolTip(
                thresholdNotificationsCheckBox,
                "Show an alert when remaining usage first reaches the low or critical threshold.");
            layout.Controls.Add(thresholdNotificationsCheckBox, 0, 1);

            FlowLayoutPanel thresholdRow = CreateFlowLayout();
            thresholdRow.Margin = new Padding(18, 0, 0, 1);
            thresholdRow.Controls.Add(CreateRowLabel("&Critical", "Critical threshold"));
            criticalNumeric = CreateNumeric(1, 99, 66, "Critical threshold percent");
            thresholdRow.Controls.Add(criticalNumeric);
            thresholdRow.Controls.Add(CreateSuffixLabel("%"));

            Label lowLabel = CreateRowLabel("&Low", "Low threshold");
            lowLabel.Margin = new Padding(16, 3, 3, 1);
            thresholdRow.Controls.Add(lowLabel);
            lowNumeric = CreateNumeric(1, 99, 66, "Low threshold percent");
            thresholdRow.Controls.Add(lowNumeric);
            thresholdRow.Controls.Add(CreateSuffixLabel("%"));
            toolTip.SetToolTip(criticalNumeric, "Critical must be less than or equal to Low. The paired value adjusts immediately.");
            toolTip.SetToolTip(lowNumeric, "Low must be greater than or equal to Critical. The paired value adjusts immediately.");
            thresholdControls = thresholdRow;
            layout.Controls.Add(thresholdRow, 0, 2);

            group.Controls.Add(layout);
            return group;
        }

        private GroupBox BuildRefreshGroup()
        {
            GroupBox group = CreateGroup("Refresh");
            TableLayoutPanel layout = CreateGroupLayout(3);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));

            Label runningLabel = CreateRowLabel("Codex &running", "Refresh interval while Codex is running");
            runningLabel.MinimumSize = new Size(112, 0);
            layout.Controls.Add(runningLabel, 0, 0);
            refreshNumeric = CreateNumeric(30, 3600, 70, "Active refresh interval in seconds");
            layout.Controls.Add(refreshNumeric, 1, 0);
            layout.Controls.Add(CreateSuffixLabel("seconds"), 2, 0);

            Label idleLabel = CreateRowLabel("Codex &closed", "Refresh interval while Codex is closed");
            idleLabel.MinimumSize = new Size(112, 0);
            layout.Controls.Add(idleLabel, 0, 1);
            idleRefreshNumeric = CreateNumeric(0, 7200, 70, "Idle refresh interval in seconds");
            layout.Controls.Add(idleRefreshNumeric, 1, 1);
            layout.Controls.Add(CreateSuffixLabel("seconds (0 = off)"), 2, 1);
            toolTip.SetToolTip(refreshNumeric, "Refresh every 30 to 3600 seconds while Codex is running.");
            toolTip.SetToolTip(idleRefreshNumeric, "Use 0 to pause checks while Codex is closed, or use a value at least as large as the running interval.");

            group.Controls.Add(layout);
            return group;
        }

        private GroupBox BuildAppearanceGroup()
        {
            GroupBox group = CreateGroup("Appearance");
            TableLayoutPanel layout = CreateGroupLayout(1);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));

            TableLayoutPanel themeRow = CreateGroupLayout(2);
            themeRow.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            themeRow.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));

            Label themeLabel = CreateRowLabel("&Theme", "Application theme");
            themeLabel.MinimumSize = new Size(70, 0);
            themeRow.Controls.Add(themeLabel, 0, 0);
            themeCombo = new ComboBox();
            themeCombo.AccessibleName = "Application theme";
            themeCombo.Anchor = AnchorStyles.Left | AnchorStyles.Right;
            themeCombo.DropDownStyle = ComboBoxStyle.DropDownList;
            themeCombo.FlatStyle = FlatStyle.Flat;
            themeCombo.Margin = new Padding(3, 1, 3, 1);
            themeCombo.Items.AddRange(new object[]
            {
                AppSettings.ThemeSystem,
                AppSettings.ThemeDark,
                AppSettings.ThemeLight
            });
            themeRow.Controls.Add(themeCombo, 1, 0);
            toolTip.SetToolTip(themeCombo, "Use the Windows app theme, dark mode, or light mode.");
            layout.Controls.Add(themeRow, 0, 0);

            showTrayBoxCheckBox = CreateCheckBox(
                "Show tray &background",
                "Show tray background");
            showTrayBoxCheckBox.Margin = new Padding(3, 3, 3, 1);
            layout.Controls.Add(showTrayBoxCheckBox, 0, 1);

            TableLayoutPanel colorLayout = CreateGroupLayout(2);
            colorLayout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            colorLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));

            Label backgroundLabel = CreateRowLabel("&Background", "Tray background color");
            backgroundLabel.MinimumSize = new Size(82, 0);
            colorLayout.Controls.Add(backgroundLabel, 0, 0);
            FlowLayoutPanel backgroundRow = CreateFlowLayout();
            trayBoxColorButton = CreateColorButton("Choose tray background color");
            trayBoxColorButton.Click += delegate { PickTrayBoxColor(); };
            backgroundRow.Controls.Add(trayBoxColorButton);

            trayBoxHexLabel = CreateValueLabel("Tray background color value");
            trayBoxAutoButton = CreateButton("&Auto", "Use automatic tray background color", 54);
            trayBoxAutoButton.Margin = new Padding(3, 1, 6, 1);
            trayBoxAutoButton.Anchor = AnchorStyles.Left;
            trayBoxAutoButton.Click += delegate
            {
                useCustomTrayBoxColor = false;
                UpdatePreviews();
            };
            backgroundRow.Controls.Add(trayBoxAutoButton);
            backgroundRow.Controls.Add(trayBoxHexLabel);
            trayBackgroundControls = backgroundRow;
            colorLayout.Controls.Add(backgroundRow, 1, 0);

            Label textLabel = CreateRowLabel("&Text", "Tray text color");
            textLabel.MinimumSize = new Size(82, 0);
            colorLayout.Controls.Add(textLabel, 0, 1);
            FlowLayoutPanel textRow = CreateFlowLayout();
            trayTextColorButton = CreateColorButton("Choose tray text color");
            trayTextColorButton.Click += delegate { PickTrayTextColor(); };
            textRow.Controls.Add(trayTextColorButton);

            trayTextHexLabel = CreateValueLabel("Tray text color value");
            textRow.Controls.Add(trayTextHexLabel);
            colorLayout.Controls.Add(textRow, 1, 1);
            layout.Controls.Add(colorLayout, 0, 2);

            TableLayoutPanel previewLayout = CreateGroupLayout(2);
            previewLayout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            previewLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));
            Label previewLabel = CreateRowLabel("Tray &preview", "Live tray preview");
            previewLabel.MinimumSize = new Size(82, 0);
            previewLabel.Margin = new Padding(3, 9, 5, 1);
            previewLayout.Controls.Add(previewLabel, 0, 0);

            FlowLayoutPanel previewRow = CreateFlowLayout();

            trayPreview = new PictureBox();
            trayPreview.AccessibleName = "Live tray preview";
            trayPreview.AccessibleRole = AccessibleRole.Graphic;
            trayPreview.BackColor = SystemColors.ControlDark;
            trayPreview.BorderStyle = BorderStyle.FixedSingle;
            trayPreview.Margin = new Padding(3, 1, 8, 1);
            trayPreview.Size = new Size(34, 34);
            trayPreview.SizeMode = PictureBoxSizeMode.CenterImage;
            previewRow.Controls.Add(trayPreview);

            Label remainingLabel = CreateRowLabel("&Remaining", "Preview remaining usage");
            remainingLabel.Margin = new Padding(3, 9, 3, 1);
            previewRow.Controls.Add(remainingLabel);
            previewPercentNumeric = CreateNumeric(0, 100, 62, "Preview remaining usage percent");
            previewPercentNumeric.Value = 50;
            previewPercentNumeric.Margin = new Padding(3, 5, 3, 1);
            previewRow.Controls.Add(previewPercentNumeric);
            Label previewSuffix = CreateSuffixLabel("%");
            previewSuffix.Margin = new Padding(0, 9, 3, 1);
            previewRow.Controls.Add(previewSuffix);
            toolTip.SetToolTip(previewPercentNumeric, "Change this sample percentage to preview normal, low, and critical tray states.");
            previewLayout.Controls.Add(previewRow, 1, 0);
            layout.Controls.Add(previewLayout, 0, 3);

            contrastStatusLabel = CreateValueLabel("Tray text contrast guidance");
            contrastStatusLabel.MaximumSize = new Size(250, 0);
            contrastStatusLabel.Margin = new Padding(88, 1, 3, 1);
            layout.Controls.Add(contrastStatusLabel, 0, 4);

            group.Controls.Add(layout);
            return group;
        }

        private GroupBox BuildPopupGroup()
        {
            GroupBox group = CreateGroup("Popup");
            TableLayoutPanel layout = CreateGroupLayout(2);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 50.0F));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 50.0F));

            showLastUpdatedCheckBox = CreateCheckBox(
                "Show &updated time",
                "Show updated time");
            layout.Controls.Add(showLastUpdatedCheckBox, 0, 0);

            showResetTimesCheckBox = CreateCheckBox(
                "Show &reset times",
                "Show reset times");
            layout.Controls.Add(showResetTimesCheckBox, 1, 0);

            showResetAvailabilityCheckBox = CreateCheckBox(
                "Show &limit resets",
                "Show available limit resets");
            toolTip.SetToolTip(
                showResetAvailabilityCheckBox,
                "Show available limit reset credits and expiration dates in the expanded popup.");
            layout.Controls.Add(showResetAvailabilityCheckBox, 0, 1);

            colorBarsCheckBox = CreateCheckBox(
                "Color &usage bars",
                "Color usage bars by remaining usage");
            toolTip.SetToolTip(colorBarsCheckBox, "Color popup bars by normal, low, and critical usage levels.");
            layout.Controls.Add(colorBarsCheckBox, 1, 1);

            group.Controls.Add(layout);
            return group;
        }

        private GroupBox BuildSystemGroup()
        {
            GroupBox group = CreateGroup("Startup & updates");
            TableLayoutPanel groupLayout = CreateGroupLayout(1);

            startWithWindowsCheckBox = CreateCheckBox(
                "&Start with Windows",
                "Start with Windows");
            toolTip.SetToolTip(startWithWindowsCheckBox, "Launch Codex Usage Tray when you sign in to Windows.");
            groupLayout.Controls.Add(startWithWindowsCheckBox, 0, 0);

            TableLayoutPanel layout = CreateGroupLayout(3);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));

            Label versionLabel = CreateValueLabel("Installed version");
            versionLabel.Text = "Version " + AppVersion.Current;
            versionLabel.Margin = new Padding(3, 4, 8, 1);
            layout.Controls.Add(versionLabel, 0, 0);

            updateStatusLabel = CreateValueLabel("Update status");
            updateStatusLabel.AutoEllipsis = true;
            updateStatusLabel.AutoSize = false;
            updateStatusLabel.Dock = DockStyle.Fill;
            updateStatusLabel.Margin = new Padding(3, 3, 8, 1);
            updateStatusLabel.MinimumSize = new Size(0, 24);
            updateStatusLabel.TextAlign = ContentAlignment.MiddleLeft;
            layout.Controls.Add(updateStatusLabel, 1, 0);

            checkUpdatesButton = CreateButton("Check &Updates", "Check for updates", 112);
            checkUpdatesButton.Click += delegate
            {
                if (CheckUpdatesRequested != null)
                {
                    CheckUpdatesRequested(this, EventArgs.Empty);
                }
            };
            layout.Controls.Add(checkUpdatesButton, 2, 0);

            groupLayout.Controls.Add(layout, 0, 1);
            group.Controls.Add(groupLayout);
            return group;
        }

        private void LoadSettings()
        {
            adjustingValues = true;
            SetNumericValue(criticalNumeric, settings.CriticalThreshold);
            SetNumericValue(lowNumeric, settings.LowThreshold);
            SetNumericValue(refreshNumeric, settings.RefreshSeconds);
            SetNumericValue(idleRefreshNumeric, settings.IdleRefreshSeconds);
            EnsureValidValues();

            weeklyRadio.Checked = !string.Equals(
                settings.IconMetric,
                AppSettings.IconMetricFiveHour,
                StringComparison.OrdinalIgnoreCase);
            fiveHourRadio.Checked = string.Equals(
                settings.IconMetric,
                AppSettings.IconMetricFiveHour,
                StringComparison.OrdinalIgnoreCase);
            colorBarsCheckBox.Checked = settings.ColorBars;
            showResetTimesCheckBox.Checked = settings.ShowPopupResetTimes;
            showLastUpdatedCheckBox.Checked = settings.ShowPopupLastUpdated;
            showResetAvailabilityCheckBox.Checked = settings.ShowResetAvailability;
            startWithWindowsCheckBox.Checked = StartupManager.IsEnabled();
            thresholdNotificationsCheckBox.Checked = settings.ThresholdNotifications;
            showTrayBoxCheckBox.Checked = settings.ShowTrayBox;
            useCustomTrayBoxColor = settings.UseCustomTrayBoxColor;
            trayBoxColor = settings.GetTrayBoxColor();
            trayTextColor = settings.GetTrayTextColor();
            SelectTheme(settings.Theme);
            adjustingValues = false;
            UpdateDependentControlStates();
        }

        private void WireEvents()
        {
            criticalNumeric.ValueChanged += delegate
            {
                if (adjustingValues)
                {
                    return;
                }
                adjustingValues = true;
                if (criticalNumeric.Value > lowNumeric.Value)
                {
                    lowNumeric.Value = criticalNumeric.Value;
                }
                adjustingValues = false;
                UpdatePreviews();
            };

            lowNumeric.ValueChanged += delegate
            {
                if (adjustingValues)
                {
                    return;
                }
                adjustingValues = true;
                if (lowNumeric.Value < criticalNumeric.Value)
                {
                    criticalNumeric.Value = lowNumeric.Value;
                }
                adjustingValues = false;
                UpdatePreviews();
            };

            refreshNumeric.ValueChanged += delegate
            {
                if (adjustingValues)
                {
                    return;
                }
                adjustingValues = true;
                if (idleRefreshNumeric.Value > 0 && idleRefreshNumeric.Value < refreshNumeric.Value)
                {
                    idleRefreshNumeric.Value = refreshNumeric.Value;
                }
                adjustingValues = false;
            };

            idleRefreshNumeric.ValueChanged += delegate
            {
                if (adjustingValues)
                {
                    return;
                }
                adjustingValues = true;
                if (idleRefreshNumeric.Value > 0 && idleRefreshNumeric.Value < refreshNumeric.Value)
                {
                    idleRefreshNumeric.Value = refreshNumeric.Value;
                }
                adjustingValues = false;
            };

            themeCombo.SelectedIndexChanged += delegate
            {
                if (!adjustingValues)
                {
                    ApplyCurrentTheme();
                    UpdatePreviews();
                }
            };
            thresholdNotificationsCheckBox.CheckedChanged += delegate { UpdateDependentControlStates(); };
            showTrayBoxCheckBox.CheckedChanged += delegate
            {
                UpdateDependentControlStates();
                UpdatePreviews();
            };
            previewPercentNumeric.ValueChanged += delegate { UpdatePreviews(); };
        }

        private bool TryApplyToSettings()
        {
            if (settings.IsSaveBlockedByNewerVersion)
            {
                MessageBox.Show(
                    this,
                    settings.SaveBlockedMessage,
                    "Codex Usage Settings",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Warning);
                return false;
            }

            EnsureValidValues();
            AppSettings candidate = settings.Clone();
            candidate.CriticalThreshold = (int)criticalNumeric.Value;
            candidate.LowThreshold = (int)lowNumeric.Value;
            candidate.IconMetric = fiveHourRadio.Checked
                ? AppSettings.IconMetricFiveHour
                : AppSettings.IconMetricWeekly;
            candidate.ColorBars = colorBarsCheckBox.Checked;
            candidate.ShowPopupResetTimes = showResetTimesCheckBox.Checked;
            candidate.ShowPopupLastUpdated = showLastUpdatedCheckBox.Checked;
            candidate.ShowResetAvailability = showResetAvailabilityCheckBox.Checked;
            candidate.StartWithWindows = startWithWindowsCheckBox.Checked;
            candidate.ThresholdNotifications = thresholdNotificationsCheckBox.Checked;
            candidate.ShowTrayBox = showTrayBoxCheckBox.Checked;
            candidate.UseCustomTrayBoxColor = useCustomTrayBoxColor;
            candidate.TrayBoxColor = AppSettings.FormatColor(trayBoxColor);
            candidate.TrayTextColor = AppSettings.FormatColor(trayTextColor);
            candidate.RefreshSeconds = (int)refreshNumeric.Value;
            candidate.IdleRefreshSeconds = (int)idleRefreshNumeric.Value;
            candidate.Theme = GetSelectedTheme();

            try
            {
                candidate.Save();
            }
            catch (Exception ex)
            {
                MessageBox.Show(
                    this,
                    "Could not save settings:" + Environment.NewLine + Environment.NewLine + ex.Message,
                    "Codex Usage Settings",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Warning);
                return false;
            }

            settings.CopyValuesFrom(candidate);

            if (SettingsApplied != null)
            {
                SettingsApplied(this, EventArgs.Empty);
            }

            return true;
        }

        private void RestoreDefaults()
        {
            AppSettings defaults = new AppSettings();
            adjustingValues = true;
            criticalNumeric.Value = defaults.CriticalThreshold;
            lowNumeric.Value = defaults.LowThreshold;
            refreshNumeric.Value = defaults.RefreshSeconds;
            idleRefreshNumeric.Value = defaults.IdleRefreshSeconds;
            weeklyRadio.Checked = true;
            fiveHourRadio.Checked = false;
            colorBarsCheckBox.Checked = defaults.ColorBars;
            showResetTimesCheckBox.Checked = defaults.ShowPopupResetTimes;
            showLastUpdatedCheckBox.Checked = defaults.ShowPopupLastUpdated;
            showResetAvailabilityCheckBox.Checked = defaults.ShowResetAvailability;
            startWithWindowsCheckBox.Checked = defaults.StartWithWindows;
            thresholdNotificationsCheckBox.Checked = defaults.ThresholdNotifications;
            showTrayBoxCheckBox.Checked = defaults.ShowTrayBox;
            useCustomTrayBoxColor = defaults.UseCustomTrayBoxColor;
            trayBoxColor = defaults.GetTrayBoxColor();
            trayTextColor = defaults.GetTrayTextColor();
            previewPercentNumeric.Value = 50;
            SelectTheme(defaults.Theme);
            adjustingValues = false;
            UpdateDependentControlStates();
            ApplyCurrentTheme();
            UpdatePreviews();
        }

        private void UpdateDependentControlStates()
        {
            if (thresholdControls != null)
            {
                thresholdControls.Enabled = thresholdNotificationsCheckBox.Checked;
            }
            if (trayBackgroundControls != null)
            {
                trayBackgroundControls.Enabled = showTrayBoxCheckBox.Checked;
            }
            if (trayBoxAutoButton != null)
            {
                trayBoxAutoButton.Enabled = showTrayBoxCheckBox.Checked && useCustomTrayBoxColor;
            }
        }

        public void SetUpdateButtonState(bool busy, string busyText)
        {
            if (checkUpdatesButton == null)
            {
                return;
            }

            checkUpdatesButton.Enabled = !busy;
            checkUpdatesButton.Text = busy ? busyText : "Check &Updates";
        }

        public void SetUpdateStatus(string status)
        {
            if (updateStatusLabel == null)
            {
                return;
            }

            updateStatusLabel.Text = status ?? "";
            updateStatusLabel.AccessibleDescription = updateStatusLabel.Text;
            toolTip.SetToolTip(updateStatusLabel, updateStatusLabel.Text);
        }

        private void PickTrayBoxColor()
        {
            Color selected;
            if (TryPickColor(trayBoxColor, out selected))
            {
                trayBoxColor = selected;
                useCustomTrayBoxColor = true;
                UpdatePreviews();
            }
        }

        private void PickTrayTextColor()
        {
            Color selected;
            if (TryPickColor(trayTextColor, out selected))
            {
                trayTextColor = selected;
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

        private void EnsureValidValues()
        {
            if (criticalNumeric.Value > lowNumeric.Value)
            {
                lowNumeric.Value = criticalNumeric.Value;
            }
            if (idleRefreshNumeric.Value > 0 && idleRefreshNumeric.Value < refreshNumeric.Value)
            {
                idleRefreshNumeric.Value = refreshNumeric.Value;
            }
        }

        private void UpdatePreviews()
        {
            if (trayPreview == null || previewPercentNumeric == null)
            {
                return;
            }

            int previewPercent = (int)previewPercentNumeric.Value;
            AppSettings previewSettings = CreatePreviewSettings();
            ReplacePreview(
                trayPreview,
                IconRenderer.CreatePreviewBitmap(previewPercent, previewSettings));

            Color effectiveBackground = useCustomTrayBoxColor
                ? trayBoxColor
                : IconRenderer.ColorForPercent(previewPercent, previewSettings);
            string effectiveBackgroundHex = AppSettings.FormatColor(effectiveBackground);
            string textHex = AppSettings.FormatColor(trayTextColor);

            trayBoxColorButton.FlatStyle = FlatStyle.Flat;
            trayBoxColorButton.UseVisualStyleBackColor = false;
            trayBoxColorButton.BackColor = effectiveBackground;
            trayBoxColorButton.ForeColor = ContrastColor(effectiveBackground);
            trayBoxColorButton.Text = useCustomTrayBoxColor ? "" : "A";
            trayBoxColorButton.AccessibleDescription = useCustomTrayBoxColor
                ? "Current tray background color " + effectiveBackgroundHex
                : "Automatic tray background color. Current preview " + effectiveBackgroundHex;

            trayTextColorButton.FlatStyle = FlatStyle.Flat;
            trayTextColorButton.UseVisualStyleBackColor = false;
            trayTextColorButton.BackColor = trayTextColor;
            trayTextColorButton.ForeColor = ContrastColor(trayTextColor);
            trayTextColorButton.Text = "";
            trayTextColorButton.AccessibleDescription = "Current tray text color " + textHex;

            trayBoxAutoButton.Enabled = showTrayBoxCheckBox.Checked && useCustomTrayBoxColor;
            trayBoxHexLabel.Text = useCustomTrayBoxColor
                ? effectiveBackgroundHex
                : "Auto (" + effectiveBackgroundHex + ")";
            trayTextHexLabel.Text = textHex;

            toolTip.SetToolTip(
                trayBoxColorButton,
                useCustomTrayBoxColor
                    ? "Choose tray background color. Current: " + effectiveBackgroundHex
                    : "Choose a fixed tray background color. Current preview is automatic: " + effectiveBackgroundHex);
            toolTip.SetToolTip(
                trayBoxAutoButton,
                "Use automatic critical, low, and normal usage colors for the tray background.");
            toolTip.SetToolTip(
                trayTextColorButton,
                "Choose tray text color. Current: " + textHex);

            bool dark = AppSettings.IsDarkTheme(GetSelectedTheme());
            trayPreview.BackColor = SystemInformation.HighContrast
                ? SystemColors.ControlDark
                : dark ? Color.FromArgb(38, 38, 38) : Color.FromArgb(232, 232, 232);
            trayPreview.AccessibleDescription = previewPercent.ToString()
                + " percent remaining; background "
                + (showTrayBoxCheckBox.Checked ? effectiveBackgroundHex : "hidden")
                + "; text "
                + textHex;
            toolTip.SetToolTip(trayPreview, trayPreview.AccessibleDescription);

            UpdateContrastGuidance(effectiveBackground);
        }

        private AppSettings CreatePreviewSettings()
        {
            AppSettings previewSettings = new AppSettings();
            previewSettings.CriticalThreshold = (int)criticalNumeric.Value;
            previewSettings.LowThreshold = (int)lowNumeric.Value;
            previewSettings.ShowTrayBox = showTrayBoxCheckBox.Checked;
            previewSettings.UseCustomTrayBoxColor = useCustomTrayBoxColor;
            previewSettings.TrayBoxColor = AppSettings.FormatColor(trayBoxColor);
            previewSettings.TrayTextColor = AppSettings.FormatColor(trayTextColor);
            return previewSettings;
        }

        private void UpdateContrastGuidance(Color effectiveBackground)
        {
            if (!showTrayBoxCheckBox.Checked)
            {
                contrastStatusLabel.Text = "Text must contrast with the taskbar.";
                contrastStatusLabel.AccessibleDescription = contrastStatusLabel.Text;
                contrastStatusLabel.ForeColor = SystemInformation.HighContrast
                    ? SystemColors.ControlText
                    : ForeColor;
                toolTip.SetToolTip(contrastStatusLabel, contrastStatusLabel.Text);
                return;
            }

            double ratio = ContrastRatio(trayTextColor, effectiveBackground);
            double blackRatio = ContrastRatio(Color.Black, effectiveBackground);
            double whiteRatio = ContrastRatio(Color.White, effectiveBackground);
            Color recommended = blackRatio >= whiteRatio ? Color.Black : Color.White;
            string recommendedHex = AppSettings.FormatColor(recommended);
            bool lowContrast = ratio < 4.5;
            string ratioText = ratio.ToString("0.00") + ":1";

            contrastStatusLabel.Text = lowContrast
                ? "Low contrast " + ratioText + ". Try " + recommendedHex + "."
                : "Contrast " + ratioText;
            contrastStatusLabel.AccessibleDescription = contrastStatusLabel.Text;
            contrastStatusLabel.ForeColor = lowContrast
                ? GetWarningColor()
                : (SystemInformation.HighContrast ? SystemColors.ControlText : ForeColor);
            toolTip.SetToolTip(
                contrastStatusLabel,
                "A contrast ratio of 4.5:1 or higher is recommended for readable tray text.");
        }

        private Color GetWarningColor()
        {
            if (SystemInformation.HighContrast)
            {
                return SystemColors.ControlText;
            }

            return AppSettings.IsDarkTheme(GetSelectedTheme())
                ? Color.FromArgb(255, 205, 86)
                : Color.DarkRed;
        }

        private static double ContrastRatio(Color first, Color second)
        {
            double firstLuminance = RelativeLuminance(first);
            double secondLuminance = RelativeLuminance(second);
            double lighter = Math.Max(firstLuminance, secondLuminance);
            double darker = Math.Min(firstLuminance, secondLuminance);
            return (lighter + 0.05) / (darker + 0.05);
        }

        private static double RelativeLuminance(Color color)
        {
            return (0.2126 * LinearColorChannel(color.R))
                + (0.7152 * LinearColorChannel(color.G))
                + (0.0722 * LinearColorChannel(color.B));
        }

        private static double LinearColorChannel(byte channel)
        {
            double value = channel / 255.0;
            return value <= 0.03928
                ? value / 12.92
                : Math.Pow((value + 0.055) / 1.055, 2.4);
        }

        private static Color ContrastColor(Color color)
        {
            int brightness = ((color.R * 299) + (color.G * 587) + (color.B * 114)) / 1000;
            return brightness > 140 ? Color.Black : Color.White;
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

        private void ApplyCurrentTheme()
        {
            ApplyTheme(this, AppSettings.IsDarkTheme(GetSelectedTheme()));
        }

        private static void ApplyTheme(Control control, bool dark)
        {
            bool highContrast = SystemInformation.HighContrast;
            Color backColor = highContrast
                ? SystemColors.Control
                : dark ? Color.FromArgb(54, 54, 54) : SystemColors.Control;
            Color foreColor = highContrast
                ? SystemColors.ControlText
                : dark ? Color.White : SystemColors.ControlText;
            Color inputBackColor = highContrast
                ? SystemColors.Window
                : dark ? Color.FromArgb(36, 36, 36) : SystemColors.Window;
            Color inputForeColor = highContrast
                ? SystemColors.WindowText
                : dark ? Color.White : SystemColors.WindowText;
            Color borderColor = highContrast
                ? SystemColors.WindowText
                : dark ? Color.FromArgb(100, 100, 100) : Color.FromArgb(170, 170, 170);

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
                if (highContrast)
                {
                    button.FlatStyle = FlatStyle.System;
                    button.UseVisualStyleBackColor = true;
                }
                else
                {
                    button.FlatStyle = FlatStyle.Flat;
                    button.UseVisualStyleBackColor = false;
                    button.BackColor = dark ? Color.FromArgb(74, 74, 74) : Color.FromArgb(245, 245, 245);
                    button.ForeColor = foreColor;
                    button.FlatAppearance.BorderColor = dark
                        ? Color.FromArgb(150, 150, 150)
                        : Color.FromArgb(170, 170, 170);
                    button.FlatAppearance.MouseOverBackColor = dark
                        ? Color.FromArgb(92, 92, 92)
                        : Color.FromArgb(229, 229, 229);
                    button.FlatAppearance.MouseDownBackColor = dark
                        ? Color.FromArgb(58, 58, 58)
                        : Color.FromArgb(214, 214, 214);
                }
            }

            foreach (Control child in control.Controls)
            {
                ApplyTheme(child, dark);
                if (child is TextBox || child is ComboBox || child is NumericUpDown)
                {
                    child.BackColor = inputBackColor;
                    child.ForeColor = inputForeColor;
                }
            }
        }

        private static GroupBox CreateGroup(string text)
        {
            GroupBox group = new DarkGroupBox();
            group.AccessibleName = text;
            group.AutoSize = true;
            group.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            group.Dock = DockStyle.Fill;
            group.Margin = new Padding(0, 0, 0, 4);
            group.Padding = new Padding(7, 3, 7, 5);
            group.Text = text;
            return group;
        }

        private static TableLayoutPanel CreateGroupLayout(int columns)
        {
            TableLayoutPanel layout = new TableLayoutPanel();
            layout.AutoSize = true;
            layout.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            layout.ColumnCount = columns;
            layout.Dock = DockStyle.Top;
            layout.GrowStyle = TableLayoutPanelGrowStyle.AddRows;
            layout.Margin = Padding.Empty;
            layout.Padding = Padding.Empty;
            return layout;
        }

        private static FlowLayoutPanel CreateFlowLayout()
        {
            FlowLayoutPanel layout = new FlowLayoutPanel();
            layout.AutoSize = true;
            layout.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            layout.FlowDirection = FlowDirection.LeftToRight;
            layout.Margin = Padding.Empty;
            layout.Padding = Padding.Empty;
            layout.WrapContents = false;
            return layout;
        }

        private static Label CreateRowLabel(string text, string accessibleName)
        {
            Label label = new Label();
            label.AccessibleName = accessibleName;
            label.Anchor = AnchorStyles.Left;
            label.AutoSize = true;
            label.Margin = new Padding(3, 3, 3, 1);
            label.Text = text;
            label.UseMnemonic = true;
            return label;
        }

        private static Label CreateSuffixLabel(string text)
        {
            Label label = new Label();
            label.Anchor = AnchorStyles.Left;
            label.AutoSize = true;
            label.Margin = new Padding(0, 3, 3, 1);
            label.Text = text;
            label.UseMnemonic = false;
            return label;
        }

        private static Label CreateValueLabel(string accessibleName)
        {
            Label label = new Label();
            label.AccessibleName = accessibleName;
            label.Anchor = AnchorStyles.Left;
            label.AutoSize = true;
            label.Margin = new Padding(3, 3, 3, 1);
            label.UseMnemonic = false;
            return label;
        }

        private static CheckBox CreateCheckBox(string text, string accessibleName)
        {
            CheckBox checkBox = new CheckBox();
            checkBox.AccessibleName = accessibleName;
            checkBox.Anchor = AnchorStyles.Left;
            checkBox.AutoSize = true;
            checkBox.Margin = new Padding(3, 1, 3, 1);
            checkBox.Text = text;
            checkBox.UseMnemonic = true;
            return checkBox;
        }

        private static RadioButton CreateRadio(string text, string accessibleName)
        {
            RadioButton radio = new RadioButton();
            radio.AccessibleName = accessibleName;
            radio.Anchor = AnchorStyles.Left;
            radio.AutoSize = true;
            radio.Margin = new Padding(3, 1, 7, 1);
            radio.Text = text;
            radio.UseMnemonic = true;
            return radio;
        }

        private static NumericUpDown CreateNumeric(
            int minimum,
            int maximum,
            int width,
            string accessibleName)
        {
            NumericUpDown numeric = new NumericUpDown();
            numeric.AccessibleName = accessibleName;
            numeric.Anchor = AnchorStyles.Left;
            numeric.Margin = new Padding(3, 1, 3, 1);
            numeric.Minimum = minimum;
            numeric.Maximum = maximum;
            numeric.Size = new Size(width, 23);
            numeric.TextAlign = HorizontalAlignment.Right;
            return numeric;
        }

        private static Button CreateButton(string text, string accessibleName, int minimumWidth)
        {
            Button button = new Button();
            button.AccessibleName = accessibleName;
            button.AutoSize = true;
            button.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            button.FlatStyle = FlatStyle.Flat;
            button.Margin = new Padding(4, 1, 0, 1);
            button.MinimumSize = new Size(minimumWidth, 28);
            button.Padding = new Padding(6, 0, 6, 0);
            button.Text = text;
            button.UseMnemonic = true;
            return button;
        }

        private static Button CreateColorButton(string accessibleName)
        {
            Button button = new Button();
            button.AccessibleName = accessibleName;
            button.Anchor = AnchorStyles.Left;
            button.FlatStyle = FlatStyle.Flat;
            button.Margin = new Padding(3, 1, 3, 1);
            button.Size = new Size(38, 28);
            button.UseVisualStyleBackColor = false;
            return button;
        }

        private static void SetNumericValue(NumericUpDown numeric, int value)
        {
            decimal clamped = Math.Max(numeric.Minimum, Math.Min(numeric.Maximum, value));
            numeric.Value = clamped;
        }
    }

    internal sealed class DarkGroupBox : GroupBox
    {
        public Color BorderColor { get; set; }

        public DarkGroupBox()
        {
            BorderColor = Color.FromArgb(100, 100, 100);
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.Clear(BackColor);
            e.Graphics.TextRenderingHint = System.Drawing.Text.TextRenderingHint.ClearTypeGridFit;

            SizeF textSize = e.Graphics.MeasureString(Text, Font);
            int textX = 8;
            int borderY = Math.Max(8, (int)(textSize.Height / 2.0F));
            int textRight = textX + (int)Math.Ceiling(textSize.Width) + 8;

            using (Pen borderPen = new Pen(BorderColor))
            using (Brush textBrush = new SolidBrush(ForeColor))
            {
                e.Graphics.DrawLine(borderPen, 1, borderY, Math.Max(1, textX - 3), borderY);
                if (textRight < Width - 2)
                {
                    e.Graphics.DrawLine(borderPen, textRight, borderY, Width - 2, borderY);
                }
                e.Graphics.DrawLine(borderPen, 1, borderY, 1, Math.Max(borderY, Height - 2));
                e.Graphics.DrawLine(borderPen, Width - 2, borderY, Width - 2, Math.Max(borderY, Height - 2));
                e.Graphics.DrawLine(borderPen, 1, Height - 2, Width - 2, Height - 2);
                e.Graphics.DrawString(Text, Font, textBrush, textX, 0);
            }
        }
    }
}
