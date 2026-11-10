using System;
using System.Drawing;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal sealed partial class SettingsForm : Form
    {
        private readonly AppSettings settings;
        private readonly ToolTip toolTip;
        private readonly Action<AppSettings> saveSettings;

        private CheckBox colorBarsCheckBox;
        private NumericUpDown criticalNumeric;
        private NumericUpDown lowNumeric;
        private NumericUpDown refreshNumeric;
        private NumericUpDown idleRefreshNumeric;
        private NumericUpDown autoRedeemLeadNumeric;
        private NumericUpDown previewPercentNumeric;
        private RadioButton weeklyRadio;
        private RadioButton fiveHourRadio;
        private RadioButton autoMetricRadio;
        private CheckBox resetExpiryRemindersCheckBox;
        private NumericUpDown resetExpiryLeadNumeric;
        private ComboBox themeCombo;
        private CheckBox showResetTimesCheckBox;
        private CheckBox showLastUpdatedCheckBox;
        private CheckBox startWithWindowsCheckBox;
        private CheckBox thresholdNotificationsCheckBox;
        private CheckBox autoRedeemResetCreditsCheckBox;
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
        private Control autoRedeemTimingControls;
        private Control trayBackgroundControls;
        private bool useCustomTrayBoxColor;
        private Color trayBoxColor;
        private Color trayTextColor;
        private bool adjustingValues;
        private RadioButton learnedRoutineRadio;
        private RadioButton recentPaceRadio;
        private ComboBox quietHoursCombo;
        private CheckBox showProjectedUsageCheckBox;
        private CheckBox highlightQuietHoursCheckBox;
        private Panel pageHost;
        private Panel sidebar;
        private Control[] pages;
        private Button[] navigationButtons;
        private Button saveButton;
        private int selectedPage;
        private bool uiInitialized;
        private bool applyingSystemTheme;
        private bool choosingColor;
        private object[] savedDraftValues;

        public event EventHandler SettingsApplied;
        public event EventHandler CheckUpdatesRequested;

        public SettingsForm(AppSettings settings) : this(settings, null)
        {
        }

        internal SettingsForm(AppSettings settings, Action<AppSettings> saveSettings)
        {
            if (settings == null)
            {
                throw new ArgumentNullException("settings");
            }

            this.settings = settings;
            this.saveSettings = saveSettings ?? delegate(AppSettings candidate) { candidate.Save(); };
            toolTip = new ToolTip();
            toolTip.AutomaticDelay = 350;
            toolTip.AutoPopDelay = 12000;
            toolTip.ShowAlways = true;

            Text = "Codex Usage Tray Settings";
            ClientSize = new Size(920, 650);
            MinimumSize = new Size(810, 560);
            FormBorderStyle = FormBorderStyle.Sizable;
            MaximizeBox = false;
            MinimizeBox = false;
            ShowIcon = true;
            ShowInTaskbar = true;
            StartPosition = FormStartPosition.CenterScreen;
            Font = new Font(SystemFonts.MessageBoxFont.FontFamily, Math.Max(10.0F, SystemFonts.MessageBoxFont.SizeInPoints));
            AutoScaleDimensions = new SizeF(96.0F, 96.0F);
            AutoScaleMode = AutoScaleMode.Dpi;

            SetWindowIcon();
            BuildUi();
            LoadSettings();
            WireEvents();
            uiInitialized = true;
            ApplyCurrentTheme();
            UpdatePreviews();
#if WPF_UI
            InitializeWpfSettings();
#endif
            savedDraftValues = GetDraftValues();
        }

        internal bool HasUnsavedChanges
        {
            get
            {
                if (choosingColor) return true;
#if WPF_UI
                if (wpfHost != null && wpfHost.Visible && wpfSettings.HasUncommittedNumericInput()) return true;
#endif
                object[] current = GetDraftValues();
                if (savedDraftValues == null || savedDraftValues.Length != current.Length) return true;
                for (int index = 0; index < current.Length; index++)
                    if (!object.Equals(savedDraftValues[index], current[index])) return true;
                return false;
            }
        }

        private object[] GetDraftValues()
        {
            return new object[]
            {
                criticalNumeric.Text, lowNumeric.Text, refreshNumeric.Text, idleRefreshNumeric.Text,
                autoRedeemLeadNumeric.Text, resetExpiryLeadNumeric.Text,
                weeklyRadio.Checked, fiveHourRadio.Checked, autoMetricRadio.Checked,
                resetExpiryRemindersCheckBox.Checked, colorBarsCheckBox.Checked,
                showResetTimesCheckBox.Checked, showLastUpdatedCheckBox.Checked, startWithWindowsCheckBox.Checked,
                thresholdNotificationsCheckBox.Checked, autoRedeemResetCreditsCheckBox.Checked,
                showTrayBoxCheckBox.Checked, useCustomTrayBoxColor, trayBoxColor.ToArgb(), trayTextColor.ToArgb(),
                themeCombo.SelectedIndex, learnedRoutineRadio.Checked, recentPaceRadio.Checked,
                quietHoursCombo.SelectedIndex, showProjectedUsageCheckBox.Checked, highlightQuietHoursCheckBox.Checked
            };
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

        protected override void OnHandleCreated(EventArgs e)
        {
            base.OnHandleCreated(e);
            ApplyWindowTitleTheme();
        }

        protected override void OnSystemColorsChanged(EventArgs e)
        {
            base.OnSystemColorsChanged(e);
            if (!uiInitialized || applyingSystemTheme || IsDisposed || Disposing) return;
            try
            {
                applyingSystemTheme = true;
                ApplyCurrentTheme();
                UpdatePreviews();
#if WPF_UI
                UpdateWpfSystemColors();
#endif
            }
            finally
            {
                applyingSystemTheme = false;
            }
        }

        private void ApplyWindowTitleTheme()
        {
            string theme = themeCombo == null ? (settings == null ? AppSettings.ThemeSystem : settings.Theme) : GetSelectedTheme();
            WindowAppearance.ApplyTitleBar(this, AppSettings.IsDarkTheme(theme));
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
            TableLayoutPanel shell = new TableLayoutPanel();
            shell.Dock = DockStyle.Fill;
            shell.ColumnCount = 2;
            shell.RowCount = 2;
            shell.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 215));
            shell.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            shell.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            shell.RowStyles.Add(new RowStyle(SizeType.Absolute, 65));

            sidebar = new Panel();
            sidebar.Dock = DockStyle.Fill;
            sidebar.Padding = new Padding(10, 24, 10, 10);
            FlowLayoutPanel navigation = new FlowLayoutPanel();
            navigation.Dock = DockStyle.Fill;
            navigation.FlowDirection = FlowDirection.TopDown;
            navigation.WrapContents = false;
            navigation.AutoScroll = false;
            navigationButtons = new Button[6];
            string[] names = { "General", "Usage & alerts", "Forecast & graphs", "Appearance", "Popup", "Updates" };
            for (int i = 0; i < names.Length; i++)
            {
                int page = i;
                Button button = new SettingsNavButton(names[i]);
                button.Width = 190;
                button.Height = Math.Max(42, Font.Height * 2 + 8);
                button.Margin = new Padding(0, 0, 0, 4);
                button.Click += delegate { SelectPage(page); };
                navigationButtons[i] = button;
                navigation.Controls.Add(button);
            }
            sidebar.Controls.Add(navigation);
            shell.Controls.Add(sidebar, 0, 0);
            shell.SetRowSpan(sidebar, 2);

            pageHost = new Panel();
            pageHost.Dock = DockStyle.Fill;
            pages = new Control[]
            {
                CreateSettingsPage("General", "Control refresh intervals and startup.", BuildGeneralGroup()),
                CreateSettingsPage("Usage & alerts", "Control how usage is shown in the tray and when you're notified.", BuildUsageGroup()),
                CreateSettingsPage("Forecast & graphs", "Choose how forecasts and usage graphs are displayed.", BuildForecastGroup()),
                CreateSettingsPage("Appearance", "Personalize the app and tray icon.", BuildAppearanceGroup()),
                CreateSettingsPage("Popup", "Choose which details appear in the tray popup.", BuildPopupGroup()),
                CreateSettingsPage("Updates", "Updates install automatically and restart the app.", BuildSystemGroup())
            };
            foreach (Control page in pages)
            {
                page.Dock = DockStyle.Fill;
                page.Visible = false;
                pageHost.Controls.Add(page);
            }
            shell.Controls.Add(pageHost, 1, 0);

            TableLayoutPanel buttonLayout = new TableLayoutPanel();
            buttonLayout.ColumnCount = 2;
            buttonLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            buttonLayout.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
            buttonLayout.Dock = DockStyle.Fill;
            buttonLayout.Padding = new Padding(24, 8, 24, 12);
            Button defaultsButton = CreateButton("Restore &defaults", "Restore default settings", 125);
            defaultsButton.Anchor = AnchorStyles.Left;
            defaultsButton.Click += delegate { RestoreDefaults(); };
            toolTip.SetToolTip(defaultsButton, "Restore the controls to their default values. Changes are saved only after Save.");
            buttonLayout.Controls.Add(defaultsButton, 0, 0);
            FlowLayoutPanel actionButtons = CreateFlowLayout();
            actionButtons.FlowDirection = FlowDirection.LeftToRight;
            actionButtons.Anchor = AnchorStyles.Right;
            Button cancelButton = CreateButton("&Cancel", "Cancel settings changes", 82);
            cancelButton.DialogResult = DialogResult.Cancel;
            cancelButton.Click += delegate { Close(); };
            actionButtons.Controls.Add(cancelButton);
            saveButton = CreateButton("&Save", "Apply settings", 82);
            saveButton.DialogResult = DialogResult.OK;
            saveButton.Click += delegate
            {
                if (!TryApplyToSettings())
                {
                    DialogResult = DialogResult.None;
                    return;
                }
                Close();
            };
            actionButtons.Controls.Add(saveButton);
            buttonLayout.Controls.Add(actionButtons, 1, 0);
            shell.Controls.Add(buttonLayout, 1, 1);
            AcceptButton = saveButton;
            CancelButton = cancelButton;
            Controls.Add(shell);
            SelectPage(1);
            ResumeLayout(false);
        }

        private Control CreateSettingsPage(string title, string description, Control content)
        {
            TableLayoutPanel page = new TableLayoutPanel();
            page.Dock = DockStyle.Fill;
            page.Padding = new Padding(24, 22, 24, 0);
            page.ColumnCount = 1;
            page.RowCount = 3;
            page.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            page.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            page.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            page.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            Label heading = new Label();
            heading.Text = title;
            heading.UseMnemonic = false;
            heading.AccessibleRole = AccessibleRole.StaticText;
            heading.AutoSize = true;
            heading.Font = new Font(Font.FontFamily, Math.Max(20.0F, Font.SizeInPoints + 7.0F), FontStyle.Bold);
            heading.Margin = new Padding(0, 0, 0, 4);
            page.Controls.Add(heading, 0, 0);
            Label subtitle = new Label();
            subtitle.Text = description;
            subtitle.AutoSize = true;
            subtitle.Margin = new Padding(0, 0, 0, 18);
            page.Controls.Add(subtitle, 0, 1);
            Panel scroll = new Panel();
            scroll.Dock = DockStyle.Fill;
            scroll.AutoScroll = true;
            GroupBox pageGroup = content as GroupBox;
            if (pageGroup != null)
            {
                pageGroup.Text = "";
                pageGroup.Tag = "page-content";
            }
            content.Dock = DockStyle.Top;
            scroll.Controls.Add(content);
            page.Controls.Add(scroll, 0, 2);
            return page;
        }

        private void SelectPage(int index)
        {
            selectedPage = index;
            for (int i = 0; i < pages.Length; i++) pages[i].Visible = i == index;
            UpdateNavigationColors();
        }

        private void UpdateNavigationColors()
        {
            if (navigationButtons == null) return;
            bool highContrast = SystemInformation.HighContrast;
            bool dark = themeCombo == null ? false : AppSettings.IsDarkTheme(GetSelectedTheme());
            for (int i = 0; i < navigationButtons.Length; i++)
            {
                Button button = navigationButtons[i];
                SettingsNavButton nav = button as SettingsNavButton;
                if (nav != null) nav.Selected = i == selectedPage;
                if (highContrast)
                {
                    button.FlatStyle = FlatStyle.System;
                    button.Invalidate();
                    continue;
                }
                button.FlatStyle = FlatStyle.Flat;
                button.UseVisualStyleBackColor = false;
                button.FlatAppearance.BorderSize = 0;
                button.BackColor = i == selectedPage ? (dark ? Color.FromArgb(31, 68, 105) : Color.FromArgb(210, 231, 250)) :
                    (dark ? Color.FromArgb(24, 29, 35) : Color.FromArgb(238, 241, 245));
                button.ForeColor = dark ? Color.White : SystemColors.ControlText;
                button.Invalidate();
            }
        }

        private Control BuildGeneralGroup()
        {
            TableLayoutPanel layout = CreateGroupLayout(1);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            layout.Controls.Add(BuildRefreshGroup(), 0, 0);
            startWithWindowsCheckBox = CreateToggle("Start with Windows", "Start with Windows");
            toolTip.SetToolTip(startWithWindowsCheckBox, "Launch Codex Usage Tray when you sign in to Windows.");
            layout.Controls.Add(CreateSettingCard("Startup", "Open the app automatically when you sign in.", startWithWindowsCheckBox), 0, 1);
            return layout;
        }

        private GroupBox BuildForecastGroup()
        {
            GroupBox group = CreateGroup("Forecast & graphs");
            TableLayoutPanel layout = CreateGroupLayout(1);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            FlowLayoutPanel method = CreateFlowLayout();
            learnedRoutineRadio = CreateSegment("Learned routine", "Learned routine forecast");
            recentPaceRadio = CreateSegment("Recent pace", "Recent pace forecast");
            method.Controls.Add(learnedRoutineRadio);
            method.Controls.Add(recentPaceRadio);
            layout.Controls.Add(CreateSettingCard("Forecast method", "Choose how future usage is estimated.", method), 0, 0);

            quietHoursCombo = new ComboBox();
            quietHoursCombo.AccessibleName = "Learned quiet hours";
            quietHoursCombo.DropDownStyle = ComboBoxStyle.DropDownList;
            quietHoursCombo.Width = 190;
            quietHoursCombo.Items.AddRange(new object[] { "Learn automatically", "Off" });
            ConfigureCombo(quietHoursCombo);
            toolTip.SetToolTip(quietHoursCombo, "Use the learned quiet-hour profile in weekly forecasts, or turn that profile off. Other patterns may still include flat periods.");
            layout.Controls.Add(CreateSettingCard("Quiet hours", "Use the learned quiet-hour profile in weekly projections.", quietHoursCombo), 0, 1);

            FlowLayoutPanel display = CreateFlowLayout();
            display.FlowDirection = FlowDirection.TopDown;
            showProjectedUsageCheckBox = CreateToggle("Show projected usage", "Show projected usage on the graph");
            highlightQuietHoursCheckBox = CreateToggle("Highlight quiet hours", "Highlight learned quiet hours on the graph");
            display.Controls.Add(showProjectedUsageCheckBox);
            display.Controls.Add(highlightQuietHoursCheckBox);
            layout.Controls.Add(CreateSettingCard("Graph display", "Choose what to show in usage graphs.", display), 0, 2);
            Label note = CreateValueLabel("Forecast guidance");
            note.Text = "These are estimates, not guarantees.";
            note.Margin = new Padding(4, 10, 4, 4);
            layout.Controls.Add(note, 0, 3);
            group.Controls.Add(layout);
            return group;
        }

        private Control CreateSettingCard(string title, string description, Control options)
        {
            SettingsCard card = new SettingsCard();
            card.AutoSize = true;
            card.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            card.Dock = DockStyle.Top;
            card.Padding = new Padding(16, 14, 16, 22);
            card.Margin = new Padding(0, 0, 0, 10);
            TableLayoutPanel layout = CreateGroupLayout(3);
            layout.RowCount = 2;
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 46));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 43));
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 57));
            SettingsCardIcon icon = new SettingsCardIcon(title);
            icon.Margin = new Padding(0, 0, 10, 0);
            layout.Controls.Add(icon, 0, 0);
            TableLayoutPanel text = CreateGroupLayout(1);
            text.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            Label label = new Label();
            label.Text = title;
            label.AutoSize = true;
            label.Font = new Font(Font.FontFamily, Math.Max(11.0F, Font.SizeInPoints + 1.0F), FontStyle.Bold);
            label.Margin = new Padding(0, 0, 4, 4);
            text.Controls.Add(label, 0, 0);
            Label hint = new Label();
            hint.Text = description;
            hint.AutoSize = true;
            hint.MaximumSize = new Size(230, 0);
            hint.Margin = new Padding(0, 0, 8, 0);
            text.Controls.Add(hint, 0, 1);
            layout.Controls.Add(text, 1, 0);
            options.Anchor = AnchorStyles.Left;
            options.Margin = new Padding(4, 4, 0, 4);
            layout.Controls.Add(options, 2, 0);
            bool stacked = false;
            card.SizeChanged += delegate
            {
                bool shouldStack = card.ClientSize.Width < 610;
                if (stacked == shouldStack) return;
                stacked = shouldStack;
                layout.SetColumnSpan(options, 1);
                layout.SetCellPosition(options, shouldStack
                    ? new TableLayoutPanelCellPosition(1, 1)
                    : new TableLayoutPanelCellPosition(2, 0));
                if (shouldStack) layout.SetColumnSpan(options, 2);
            };
            card.Controls.Add(layout);
            return card;
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
            autoMetricRadio = CreateRadio("&Auto", "Show the lowest remaining usage");
            metricOptions.Controls.Add(weeklyRadio);
            metricOptions.Controls.Add(fiveHourRadio);
            metricOptions.Controls.Add(autoMetricRadio);
            toolTip.SetToolTip(autoMetricRadio, "Show whichever limit has the lower remaining percentage. The tray tooltip identifies the selected limit.");
            toolTip.SetToolTip(weeklyRadio, "Show weekly usage remaining in the tray icon.");
            toolTip.SetToolTip(
                fiveHourRadio,
                "Show five-hour usage remaining. Weekly usage is used when the five-hour limit is unavailable.");
            metricRow.Controls.Add(metricOptions, 1, 0);
            layout.Controls.Add(metricRow, 0, 0);

            TableLayoutPanel alertOptions = CreateGroupLayout(2);
            alertOptions.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 42.0F));
            alertOptions.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 58.0F));

            thresholdNotificationsCheckBox = CreateCheckBox(
                "&Usage threshold alerts",
                "Usage threshold alerts");
            thresholdNotificationsCheckBox.Margin = new Padding(3, 3, 3, 1);
            toolTip.SetToolTip(
                thresholdNotificationsCheckBox,
                "Show an alert when remaining usage first reaches the low or critical threshold.");
            alertOptions.Controls.Add(thresholdNotificationsCheckBox, 0, 0);

            FlowLayoutPanel autoRedeemOptions = CreateFlowLayout();
            autoRedeemResetCreditsCheckBox = CreateCheckBox(
                "&Auto-use resets",
                "Automatically use expiring limit resets");
            autoRedeemResetCreditsCheckBox.Margin = new Padding(3, 3, 2, 1);
            autoRedeemOptions.Controls.Add(autoRedeemResetCreditsCheckBox);

            FlowLayoutPanel autoRedeemTiming = CreateFlowLayout();
            autoRedeemLeadNumeric = CreateNumeric(
                1,
                120,
                44,
                "Minutes before reset credit expiration");
            autoRedeemTiming.Controls.Add(autoRedeemLeadNumeric);
            autoRedeemTiming.Controls.Add(CreateSuffixLabel("min"));
            autoRedeemTimingControls = autoRedeemTiming;
            autoRedeemOptions.Controls.Add(autoRedeemTiming);
            toolTip.SetToolTip(
                autoRedeemResetCreditsCheckBox,
                "Use an available Codex reset credit shortly before it expires, but only when there is usage to reset.");
            toolTip.SetToolTip(
                autoRedeemLeadNumeric,
                "How many minutes before expiration automatic redemption may begin.");
            alertOptions.Controls.Add(autoRedeemOptions, 1, 0);
            layout.Controls.Add(alertOptions, 0, 1);

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

            FlowLayoutPanel reminderRow = CreateFlowLayout();
            resetExpiryRemindersCheckBox = CreateCheckBox("Reset expiry &reminders", "Reset expiry reminders");
            reminderRow.Controls.Add(resetExpiryRemindersCheckBox);
            resetExpiryLeadNumeric = CreateNumeric(1, 72, 44, "Hours before reset credit expiration");
            reminderRow.Controls.Add(resetExpiryLeadNumeric);
            reminderRow.Controls.Add(CreateSuffixLabel("hours before expiry"));
            toolTip.SetToolTip(resetExpiryRemindersCheckBox, "Notify once per reset credit before it expires, including when Auto-use resets is off. Known expirations trigger a fresh check even while idle.");
            layout.Controls.Add(reminderRow, 0, 3);

            // Preserve the original controls and validation while presenting each
            // setting as its own scannable card.
            FlowLayoutPanel thresholdOptions = CreateFlowLayout();
            thresholdOptions.FlowDirection = FlowDirection.TopDown;
            thresholdRow.Margin = new Padding(0, 2, 0, 0);
            thresholdOptions.Controls.Add(thresholdNotificationsCheckBox);
            thresholdOptions.Controls.Add(thresholdRow);
            Control reminderSuffix = reminderRow.Controls[2];
            FlowLayoutPanel reminderTiming = CreateFlowLayout();
            reminderTiming.Controls.Add(resetExpiryLeadNumeric);
            reminderTiming.Controls.Add(reminderSuffix);
            reminderRow.FlowDirection = FlowDirection.TopDown;
            reminderRow.Controls.Add(reminderTiming);
            layout.Controls.Clear();
            layout.Controls.Add(CreateSettingCard("Tray icon display", "Choose which limit the tray icon shows.", metricOptions), 0, 0);
            layout.Controls.Add(CreateSettingCard("Usage threshold alerts", "Get notified when usage reaches these levels.", thresholdOptions), 0, 1);
            layout.Controls.Add(CreateSettingCard("Reset expiry reminders", "Get a reminder before a reset credit expires.", reminderRow), 0, 2);
            layout.Controls.Add(CreateSettingCard("Auto-use resets", "Use an expiring reset credit when there is usage to reset.", autoRedeemOptions), 0, 3);

            group.Controls.Add(layout);
            return group;
        }

        private Control BuildRefreshGroup()
        {
            refreshNumeric = CreateNumeric(30, 3600, 70, "Active refresh interval in seconds");
            idleRefreshNumeric = CreateNumeric(0, 7200, 70, "Idle refresh interval in seconds");
            toolTip.SetToolTip(refreshNumeric, "Refresh every 30 to 3600 seconds while Codex is running.");
            toolTip.SetToolTip(idleRefreshNumeric, "Use 0 to pause checks while Codex is closed, or use a value at least as large as the running interval.");

            FlowLayoutPanel activeOptions = CreateFlowLayout();
            activeOptions.Controls.Add(refreshNumeric);
            activeOptions.Controls.Add(CreateSuffixLabel("seconds"));
            FlowLayoutPanel idleOptions = CreateFlowLayout();
            idleOptions.Controls.Add(idleRefreshNumeric);
            idleOptions.Controls.Add(CreateSuffixLabel("seconds (0 = off)"));
            TableLayoutPanel cards = CreateGroupLayout(1);
            cards.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            cards.Controls.Add(CreateSettingCard("Active refresh", "How often to refresh while Codex is running.", activeOptions), 0, 0);
            cards.Controls.Add(CreateSettingCard("Idle refresh", "How often to refresh while Codex is closed.", idleOptions), 0, 1);
            return cards;
        }

        private GroupBox BuildAppearanceGroup()
        {
            GroupBox group = CreateGroup("Appearance");
            TableLayoutPanel layout = CreateGroupLayout(1);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100.0F));

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
            ConfigureCombo(themeCombo);
            toolTip.SetToolTip(themeCombo, "Use the Windows app theme, dark mode, or light mode.");

            showTrayBoxCheckBox = CreateToggle(
                "Show tray background",
                "Show tray background");
            showTrayBoxCheckBox.Margin = new Padding(3, 3, 3, 1);
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
            FlowLayoutPanel textRow = CreateFlowLayout();
            trayTextColorButton = CreateColorButton("Choose tray text color");
            trayTextColorButton.Click += delegate { PickTrayTextColor(); };
            textRow.Controls.Add(trayTextColorButton);

            trayTextHexLabel = CreateValueLabel("Tray text color value");
            textRow.Controls.Add(trayTextHexLabel);
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
            contrastStatusLabel = CreateValueLabel("Tray text contrast guidance");
            contrastStatusLabel.MaximumSize = new Size(250, 0);
            themeCombo.Width = 210;
            FlowLayoutPanel backgroundOptions = CreateFlowLayout();
            backgroundOptions.FlowDirection = FlowDirection.TopDown;
            backgroundOptions.Controls.Add(showTrayBoxCheckBox);
            backgroundOptions.Controls.Add(backgroundRow);
            FlowLayoutPanel previewOptions = CreateFlowLayout();
            previewOptions.FlowDirection = FlowDirection.TopDown;
            contrastStatusLabel.Margin = new Padding(3, 4, 3, 1);
            previewOptions.Controls.Add(previewRow);
            previewOptions.Controls.Add(contrastStatusLabel);
            layout.Controls.Add(CreateSettingCard("Theme", "Choose the app's color scheme.", themeCombo), 0, 0);
            layout.Controls.Add(CreateSettingCard("Tray background", "Choose whether to show a background and customize its color.", backgroundOptions), 0, 1);
            layout.Controls.Add(CreateSettingCard("Tray text color", "Choose the color used for the tray percentage.", textRow), 0, 2);
            layout.Controls.Add(CreateSettingCard("Tray preview", "Preview the icon and check its contrast.", previewOptions), 0, 3);

            group.Controls.Add(layout);
            return group;
        }

        private GroupBox BuildPopupGroup()
        {
            GroupBox group = CreateGroup("Popup");
            TableLayoutPanel layout = CreateGroupLayout(1);
            layout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));

            showLastUpdatedCheckBox = CreateToggle(
                "Show updated time",
                "Show updated time");

            showResetTimesCheckBox = CreateToggle(
                "Show reset times",
                "Show reset times");

            colorBarsCheckBox = CreateToggle(
                "Color usage bars",
                "Color usage bars by remaining usage");
            toolTip.SetToolTip(colorBarsCheckBox, "Color popup bars by normal, low, and critical usage levels.");
            layout.Controls.Add(CreateSettingCard("Updated time", "Show when usage was last refreshed.", showLastUpdatedCheckBox), 0, 0);
            layout.Controls.Add(CreateSettingCard("Reset times", "Show when each usage limit resets.", showResetTimesCheckBox), 0, 1);
            layout.Controls.Add(CreateSettingCard("Usage bar colors", "Use threshold colors to show the remaining usage level.", colorBarsCheckBox), 0, 2);

            group.Controls.Add(layout);
            return group;
        }

        private GroupBox BuildSystemGroup()
        {
            GroupBox group = CreateGroup("Startup & updates");
            TableLayoutPanel groupLayout = CreateGroupLayout(1);

            FlowLayoutPanel updateOptions = CreateFlowLayout();
            updateOptions.FlowDirection = FlowDirection.TopDown;

            Label versionLabel = CreateValueLabel("Installed version");
            versionLabel.Text = "Version " + AppVersion.Current;
            versionLabel.Margin = new Padding(3, 4, 8, 1);
            updateOptions.Controls.Add(versionLabel);

            updateStatusLabel = CreateValueLabel("Update status");
            updateStatusLabel.AutoEllipsis = true;
            updateStatusLabel.AutoSize = false;
            updateStatusLabel.Width = 230;
            updateStatusLabel.Margin = new Padding(3, 3, 8, 1);
            updateStatusLabel.MinimumSize = new Size(0, 24);
            updateStatusLabel.TextAlign = ContentAlignment.MiddleLeft;
            updateOptions.Controls.Add(updateStatusLabel);

            checkUpdatesButton = CreateButton("Check &Updates", "Check for updates", 112);
            checkUpdatesButton.Click += delegate
            {
                if (CheckUpdatesRequested != null)
                {
                    CheckUpdatesRequested(this, EventArgs.Empty);
                }
            };
            updateOptions.Controls.Add(checkUpdatesButton);

            groupLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            groupLayout.Controls.Add(CreateSettingCard("Application updates", "Updates install automatically and restart the app.", updateOptions), 0, 0);
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
            SetNumericValue(autoRedeemLeadNumeric, settings.AutoRedeemLeadMinutes);
            EnsureValidValues();

            fiveHourRadio.Checked = string.Equals(
                settings.IconMetric,
                AppSettings.IconMetricFiveHour,
                StringComparison.OrdinalIgnoreCase);
            autoMetricRadio.Checked = string.Equals(settings.IconMetric, AppSettings.IconMetricAuto, StringComparison.OrdinalIgnoreCase);
            weeklyRadio.Checked = !fiveHourRadio.Checked && !autoMetricRadio.Checked;
            resetExpiryRemindersCheckBox.Checked = settings.ResetExpiryReminders;
            SetNumericValue(resetExpiryLeadNumeric, settings.ResetExpiryLeadHours);
            colorBarsCheckBox.Checked = settings.ColorBars;
            showResetTimesCheckBox.Checked = settings.ShowPopupResetTimes;
            showLastUpdatedCheckBox.Checked = settings.ShowPopupLastUpdated;
            startWithWindowsCheckBox.Checked = StartupManager.IsEnabled();
            thresholdNotificationsCheckBox.Checked = settings.ThresholdNotifications;
            autoRedeemResetCreditsCheckBox.Checked = settings.AutoRedeemResetCredits;
            showTrayBoxCheckBox.Checked = settings.ShowTrayBox;
            useCustomTrayBoxColor = settings.UseCustomTrayBoxColor;
            trayBoxColor = settings.GetTrayBoxColor();
            trayTextColor = settings.GetTrayTextColor();
            SelectTheme(settings.Theme);
            recentPaceRadio.Checked = settings.ForecastMethod == AppSettings.ForecastRecentPace;
            learnedRoutineRadio.Checked = !recentPaceRadio.Checked;
            quietHoursCombo.SelectedIndex = settings.QuietHoursMode == AppSettings.QuietHoursOff ? 1 : 0;
            showProjectedUsageCheckBox.Checked = settings.ShowProjectedUsage;
            highlightQuietHoursCheckBox.Checked = settings.HighlightQuietHours;
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
            autoRedeemResetCreditsCheckBox.CheckedChanged += delegate { UpdateDependentControlStates(); };
            resetExpiryRemindersCheckBox.CheckedChanged += delegate { UpdateDependentControlStates(); };
            learnedRoutineRadio.CheckedChanged += delegate { UpdateDependentControlStates(); };
            showProjectedUsageCheckBox.CheckedChanged += delegate { UpdateDependentControlStates(); };
            quietHoursCombo.SelectedIndexChanged += delegate { UpdateDependentControlStates(); };
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
            candidate.IconMetric = autoMetricRadio.Checked ? AppSettings.IconMetricAuto : fiveHourRadio.Checked
                ? AppSettings.IconMetricFiveHour
                : AppSettings.IconMetricWeekly;
            candidate.ColorBars = colorBarsCheckBox.Checked;
            candidate.ShowPopupResetTimes = showResetTimesCheckBox.Checked;
            candidate.ShowPopupLastUpdated = showLastUpdatedCheckBox.Checked;
            candidate.ResetExpiryReminders = resetExpiryRemindersCheckBox.Checked;
            candidate.ResetExpiryLeadHours = (int)resetExpiryLeadNumeric.Value;
            candidate.StartWithWindows = startWithWindowsCheckBox.Checked;
            candidate.ThresholdNotifications = thresholdNotificationsCheckBox.Checked;
            candidate.AutoRedeemResetCredits = autoRedeemResetCreditsCheckBox.Checked;
            candidate.AutoRedeemLeadMinutes = (int)autoRedeemLeadNumeric.Value;
            candidate.ShowTrayBox = showTrayBoxCheckBox.Checked;
            candidate.UseCustomTrayBoxColor = useCustomTrayBoxColor;
            candidate.TrayBoxColor = AppSettings.FormatColor(trayBoxColor);
            candidate.TrayTextColor = AppSettings.FormatColor(trayTextColor);
            candidate.RefreshSeconds = (int)refreshNumeric.Value;
            candidate.IdleRefreshSeconds = (int)idleRefreshNumeric.Value;
            candidate.Theme = GetSelectedTheme();
            candidate.ForecastMethod = recentPaceRadio.Checked ? AppSettings.ForecastRecentPace : AppSettings.ForecastLearnedRoutine;
            candidate.QuietHoursMode = quietHoursCombo.SelectedIndex == 1 ? AppSettings.QuietHoursOff : AppSettings.QuietHoursAutomatic;
            candidate.ShowProjectedUsage = showProjectedUsageCheckBox.Checked;
            candidate.HighlightQuietHours = highlightQuietHoursCheckBox.Checked;

            try
            {
                saveSettings(candidate);
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
            savedDraftValues = GetDraftValues();

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
            autoMetricRadio.Checked = false;
            resetExpiryRemindersCheckBox.Checked = defaults.ResetExpiryReminders;
            resetExpiryLeadNumeric.Value = defaults.ResetExpiryLeadHours;
            colorBarsCheckBox.Checked = defaults.ColorBars;
            showResetTimesCheckBox.Checked = defaults.ShowPopupResetTimes;
            showLastUpdatedCheckBox.Checked = defaults.ShowPopupLastUpdated;
            startWithWindowsCheckBox.Checked = defaults.StartWithWindows;
            thresholdNotificationsCheckBox.Checked = defaults.ThresholdNotifications;
            autoRedeemResetCreditsCheckBox.Checked = defaults.AutoRedeemResetCredits;
            autoRedeemLeadNumeric.Value = defaults.AutoRedeemLeadMinutes;
            showTrayBoxCheckBox.Checked = defaults.ShowTrayBox;
            useCustomTrayBoxColor = defaults.UseCustomTrayBoxColor;
            trayBoxColor = defaults.GetTrayBoxColor();
            trayTextColor = defaults.GetTrayTextColor();
            previewPercentNumeric.Value = 50;
            SelectTheme(defaults.Theme);
            learnedRoutineRadio.Checked = true;
            recentPaceRadio.Checked = false;
            quietHoursCombo.SelectedIndex = 0;
            showProjectedUsageCheckBox.Checked = defaults.ShowProjectedUsage;
            highlightQuietHoursCheckBox.Checked = defaults.HighlightQuietHours;
            adjustingValues = false;
            UpdateDependentControlStates();
            ApplyCurrentTheme();
            UpdatePreviews();
        }

        private void UpdateDependentControlStates()
        {
            quietHoursCombo.Enabled = learnedRoutineRadio.Checked;
            highlightQuietHoursCheckBox.Enabled = showProjectedUsageCheckBox.Checked && learnedRoutineRadio.Checked && quietHoursCombo.SelectedIndex == 0;
            resetExpiryLeadNumeric.Enabled = resetExpiryRemindersCheckBox.Checked;
            if (thresholdControls != null)
            {
                thresholdControls.Enabled = thresholdNotificationsCheckBox.Checked;
            }
            if (autoRedeemTimingControls != null)
            {
                autoRedeemTimingControls.Enabled = autoRedeemResetCreditsCheckBox.Checked;
            }
            if (trayBackgroundControls != null)
            {
                trayBackgroundControls.Enabled = showTrayBoxCheckBox.Checked;
            }
            if (trayBoxAutoButton != null)
            {
                trayBoxAutoButton.Enabled = showTrayBoxCheckBox.Checked && useCustomTrayBoxColor;
            }
            UpdateDisabledInputContrast();
        }

        private void UpdateDisabledInputContrast()
        {
            if (criticalNumeric == null || themeCombo == null) return;
            bool dark = AppSettings.IsDarkTheme(GetSelectedTheme()) && !SystemInformation.HighContrast;
            SetNumericContrast(criticalNumeric, thresholdNotificationsCheckBox.Checked, dark);
            SetNumericContrast(lowNumeric, thresholdNotificationsCheckBox.Checked, dark);
            SetNumericContrast(autoRedeemLeadNumeric, autoRedeemResetCreditsCheckBox.Checked, dark);
            SetNumericContrast(resetExpiryLeadNumeric, resetExpiryRemindersCheckBox.Checked, dark);
        }

        private static void SetNumericContrast(NumericUpDown numeric, bool enabled, bool dark)
        {
            numeric.BackColor = dark && !enabled ? Color.FromArgb(220, 225, 230) :
                dark ? Color.FromArgb(31, 36, 43) : SystemColors.Window;
            numeric.ForeColor = dark && enabled ? Color.White : SystemColors.WindowText;
        }

        public void SetUpdateButtonState(bool busy, string busyText)
        {
            if (checkUpdatesButton == null)
            {
                return;
            }

            checkUpdatesButton.Enabled = !busy;
            checkUpdatesButton.Text = busy ? busyText : "Check &Updates";
#if WPF_UI
            RefreshWpfSettingsStatus();
#endif
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
#if WPF_UI
            RefreshWpfSettingsStatus();
#endif
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
                DialogResult result;
                choosingColor = true;
                try { result = dialog.ShowDialog(this); }
                finally { choosingColor = false; }
                if (result == DialogResult.OK)
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
            ApplyWindowTitleTheme();
            UpdateNavigationColors();
            UpdateDisabledInputContrast();
            if (saveButton != null && !SystemInformation.HighContrast)
            {
                saveButton.BackColor = Color.FromArgb(0, 112, 205);
                saveButton.ForeColor = Color.White;
                saveButton.FlatAppearance.BorderColor = Color.FromArgb(30, 155, 245);
            }
        }

        private void ApplyTheme(Control control, bool dark)
        {
            bool highContrast = SystemInformation.HighContrast;
            Color backColor = highContrast
                ? SystemColors.Control
                : dark ? Color.FromArgb(19, 24, 31) : Color.FromArgb(248, 249, 251);
            if (!highContrast && IsWithin(control, sidebar))
                backColor = dark ? Color.FromArgb(24, 29, 35) : Color.FromArgb(238, 241, 245);
            if (!highContrast && IsWithinCard(control))
                backColor = dark ? Color.FromArgb(31, 36, 43) : Color.White;
            Color foreColor = highContrast
                ? SystemColors.ControlText
                : dark ? Color.White : SystemColors.ControlText;
            Color inputBackColor = highContrast
                ? SystemColors.Window
                : dark ? Color.FromArgb(31, 36, 43) : SystemColors.Window;
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
                group.BorderColor = object.Equals(group.Tag, "page-content") ? backColor : borderColor;
                group.Invalidate();
            }
            SettingsCard card = control as SettingsCard;
            if (card != null)
            {
                card.BorderColor = highContrast ? SystemColors.WindowText :
                    dark ? Color.FromArgb(73, 82, 93) : Color.FromArgb(208, 215, 223);
                card.Invalidate();
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

        private static bool IsWithin(Control control, Control ancestor)
        {
            if (ancestor == null) return false;
            while (control != null)
            {
                if (control == ancestor) return true;
                control = control.Parent;
            }
            return false;
        }

        private static bool IsWithinCard(Control control)
        {
            while (control != null)
            {
                if (control is SettingsCard) return true;
                control = control.Parent;
            }
            return false;
        }

        private static RadioButton CreateSegment(string text, string accessibleName)
        {
            SettingsSegment segment = new SettingsSegment();
            segment.Text = text;
            segment.AccessibleName = accessibleName;
            return segment;
        }

        private static void ConfigureCombo(ComboBox combo)
        {
            combo.FlatStyle = FlatStyle.Flat;
            combo.DrawMode = DrawMode.OwnerDrawFixed;
            combo.ItemHeight = Math.Max(22, combo.Font.Height + 6);
            combo.DrawItem += delegate(object sender, DrawItemEventArgs e)
            {
                DrawComboItem(combo, e, SystemInformation.HighContrast);
            };
            combo.FontChanged += delegate { combo.ItemHeight = Math.Max(22, combo.Font.Height + 6); };
        }

        internal static void DrawComboItem(ComboBox combo, DrawItemEventArgs e, bool highContrast)
        {
            bool selected = (e.State & DrawItemState.Selected) != 0 &&
                (e.State & DrawItemState.ComboBoxEdit) == 0;
            Color background = selected ? (highContrast ? SystemColors.Highlight :
                combo.BackColor.GetBrightness() < 0.5F ? Color.FromArgb(31, 68, 105) : Color.FromArgb(210, 231, 250)) : combo.BackColor;
            Color foreground = selected && highContrast ? SystemColors.HighlightText : combo.ForeColor;
            if ((e.State & DrawItemState.Disabled) != 0) foreground = SystemColors.GrayText;
            using (Brush brush = new SolidBrush(background)) e.Graphics.FillRectangle(brush, e.Bounds);
            object item = e.Index >= 0 && e.Index < combo.Items.Count ? combo.Items[e.Index] : combo.SelectedItem;
            if (item != null)
                TextRenderer.DrawText(e.Graphics, combo.GetItemText(item), e.Font ?? combo.Font,
                    Rectangle.Inflate(e.Bounds, -4, 0), foreground,
                    TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis | TextFormatFlags.NoPrefix);
            e.DrawFocusRectangle();
        }

        private static CheckBox CreateToggle(string text, string accessibleName)
        {
            SettingsToggle toggle = new SettingsToggle();
            toggle.Text = text;
            toggle.AccessibleName = accessibleName;
            return toggle;
        }

        private static GroupBox CreateGroup(string text)
        {
            GroupBox group = new DarkGroupBox();
            group.AccessibleName = text;
            group.AutoSize = true;
            group.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            group.Dock = DockStyle.Fill;
            group.Margin = new Padding(0, 0, 0, 6);
            group.Padding = new Padding(8, 4, 8, 7);
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
            Label label = new SettingsTextLabel();
            label.AccessibleName = accessibleName;
            label.Anchor = AnchorStyles.Left;
            label.AutoSize = true;
            label.Margin = new Padding(3, 4, 5, 2);
            label.Text = text;
            label.UseMnemonic = true;
            return label;
        }

        private static Label CreateSuffixLabel(string text)
        {
            Label label = new SettingsTextLabel();
            label.Anchor = AnchorStyles.Left;
            label.AutoSize = true;
            label.Margin = new Padding(0, 4, 4, 2);
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
            checkBox.Margin = new Padding(3, 2, 5, 2);
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
            radio.Margin = new Padding(3, 2, 9, 2);
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
            numeric.Margin = new Padding(3, 2, 3, 2);
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

    internal sealed class SettingsTextLabel : Label
    {
        protected override void OnPaint(PaintEventArgs e)
        {
            if (Enabled || SystemInformation.HighContrast)
            {
                base.OnPaint(e);
                return;
            }
            Color color = BackColor.GetBrightness() < 0.5F ? Color.FromArgb(174, 185, 197) : Color.FromArgb(90, 100, 110);
            TextRenderer.DrawText(e.Graphics, Text.Replace("&", ""), Font, ClientRectangle, color,
                TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.NoPrefix);
        }
    }

    internal static class SettingsDrawing
    {
        public static System.Drawing.Drawing2D.GraphicsPath Rounded(Rectangle bounds, int radius)
        {
            System.Drawing.Drawing2D.GraphicsPath path = new System.Drawing.Drawing2D.GraphicsPath();
            int diameter = Math.Min(radius * 2, Math.Min(bounds.Width, bounds.Height));
            if (diameter <= 0) return path;
            path.AddArc(bounds.Left, bounds.Top, diameter, diameter, 180, 90);
            path.AddArc(bounds.Right - diameter, bounds.Top, diameter, diameter, 270, 90);
            path.AddArc(bounds.Right - diameter, bounds.Bottom - diameter, diameter, diameter, 0, 90);
            path.AddArc(bounds.Left, bounds.Bottom - diameter, diameter, diameter, 90, 90);
            path.CloseFigure();
            return path;
        }
    }

    internal sealed class SettingsNavButton : Button
    {
        public bool Selected { get; set; }
        private bool hovered;

        public SettingsNavButton(string text)
        {
            Text = text;
            AccessibleName = text;
            UseMnemonic = false;
            TabStop = true;
            SetStyle(ControlStyles.UserPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
        }

        protected override void OnMouseEnter(EventArgs e) { hovered = true; Invalidate(); base.OnMouseEnter(e); }
        protected override void OnMouseLeave(EventArgs e) { hovered = false; Invalidate(); base.OnMouseLeave(e); }
        protected override void OnGotFocus(EventArgs e) { base.OnGotFocus(e); Invalidate(); }
        protected override void OnLostFocus(EventArgs e) { base.OnLostFocus(e); Invalidate(); }

        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            e.Graphics.Clear(Parent == null ? BackColor : Parent.BackColor);
            bool highContrast = SystemInformation.HighContrast;
            Color fill = highContrast ? (Selected ? SystemColors.Highlight : SystemColors.Control) : BackColor;
            if (Selected || hovered)
            {
                using (System.Drawing.Drawing2D.GraphicsPath path = SettingsDrawing.Rounded(new Rectangle(0, 0, Width - 1, Height - 1), 7))
                using (Brush brush = new SolidBrush(fill)) e.Graphics.FillPath(brush, path);
            }
            Color textColor = highContrast ? (Selected ? SystemColors.HighlightText : SystemColors.ControlText) : ForeColor;
            using (Pen pen = new Pen(textColor, 1.6F))
            {
                int x = 12, y = (Height - 18) / 2;
                if (Text == "General")
                {
                    e.Graphics.DrawEllipse(pen, x + 3, y + 3, 12, 12);
                    e.Graphics.DrawEllipse(pen, x + 7, y + 7, 4, 4);
                    e.Graphics.DrawLine(pen, x + 9, y, x + 9, y + 3);
                    e.Graphics.DrawLine(pen, x + 9, y + 15, x + 9, y + 18);
                    e.Graphics.DrawLine(pen, x, y + 9, x + 3, y + 9);
                    e.Graphics.DrawLine(pen, x + 15, y + 9, x + 18, y + 9);
                }
                else if (Text == "Usage & alerts" || Text == "Forecast & graphs")
                {
                    e.Graphics.DrawLine(pen, x + 2, y + 16, x + 2, y + 11);
                    e.Graphics.DrawLine(pen, x + 8, y + 16, x + 8, y + 6);
                    e.Graphics.DrawLine(pen, x + 14, y + 16, x + 14, y + 1);
                }
                else if (Text == "Appearance")
                {
                    e.Graphics.DrawLine(pen, x + 3, y + 15, x + 14, y + 3);
                    e.Graphics.DrawEllipse(pen, x + 1, y + 13, 5, 4);
                }
                else if (Text == "Popup")
                {
                    e.Graphics.DrawRectangle(pen, x + 1, y + 2, 16, 12);
                    e.Graphics.DrawLine(pen, x + 5, y + 14, x + 5, y + 17);
                    e.Graphics.DrawLine(pen, x + 5, y + 17, x + 9, y + 14);
                }
                else
                {
                    e.Graphics.DrawArc(pen, x + 1, y + 1, 16, 16, 40, 280);
                    e.Graphics.DrawLine(pen, x + 14, y + 1, x + 17, y + 4);
                    e.Graphics.DrawLine(pen, x + 17, y + 4, x + 13, y + 5);
                }
            }
            TextRenderer.DrawText(e.Graphics, Text, Font, new Rectangle(42, 0, Width - 45, Height), textColor,
                TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.WordBreak | TextFormatFlags.NoPrefix);
            if (Focused && ShowFocusCues) ControlPaint.DrawFocusRectangle(e.Graphics, new Rectangle(3, 3, Width - 6, Height - 6));
        }
    }

    internal sealed class SettingsSegment : RadioButton
    {
        public SettingsSegment()
        {
            Appearance = Appearance.Button;
            AutoSize = false;
            Size = new Size(125, 34);
            Margin = new Padding(0);
            TextAlign = ContentAlignment.MiddleCenter;
            SetStyle(ControlStyles.UserPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
        }

        protected override void OnCheckedChanged(EventArgs e) { base.OnCheckedChanged(e); Invalidate(); }
        protected override void OnGotFocus(EventArgs e) { base.OnGotFocus(e); Invalidate(); }
        protected override void OnLostFocus(EventArgs e) { base.OnLostFocus(e); Invalidate(); }
        protected override void OnFontChanged(EventArgs e) { base.OnFontChanged(e); FitText(); }
        protected override void OnTextChanged(EventArgs e) { base.OnTextChanged(e); FitText(); }

        private void FitText()
        {
            Size measured = TextRenderer.MeasureText(Text, Font);
            Size = new Size(Math.Max(125, measured.Width + 18), Math.Max(34, measured.Height + 12));
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            e.Graphics.Clear(Parent == null ? BackColor : Parent.BackColor);
            bool highContrast = SystemInformation.HighContrast;
            Color fill = highContrast ? (Checked ? SystemColors.Highlight : SystemColors.Control) :
                Checked ? Color.FromArgb(0, 112, 205) : BackColor;
            Color border = highContrast ? SystemColors.ControlText : Checked ? Color.FromArgb(45, 164, 250) : Color.FromArgb(100, 111, 123);
            using (System.Drawing.Drawing2D.GraphicsPath path = SettingsDrawing.Rounded(new Rectangle(1, 1, Width - 3, Height - 3), 6))
            using (Brush brush = new SolidBrush(fill))
            using (Pen pen = new Pen(border))
            {
                e.Graphics.FillPath(brush, path);
                e.Graphics.DrawPath(pen, path);
            }
            Color text = highContrast ? (Checked ? SystemColors.HighlightText : SystemColors.ControlText) : Checked ? Color.White : ForeColor;
            TextRenderer.DrawText(e.Graphics, Text, Font, ClientRectangle, text,
                TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis | TextFormatFlags.NoPrefix);
            if (Focused && ShowFocusCues) ControlPaint.DrawFocusRectangle(e.Graphics, new Rectangle(4, 4, Width - 8, Height - 8));
        }
    }

    internal sealed class SettingsToggle : CheckBox
    {
        public SettingsToggle()
        {
            AutoSize = false;
            Size = new Size(270, 32);
            Margin = new Padding(3, 1, 3, 1);
            SetStyle(ControlStyles.UserPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
        }

        protected override void OnCheckedChanged(EventArgs e) { base.OnCheckedChanged(e); Invalidate(); }
        protected override void OnGotFocus(EventArgs e) { base.OnGotFocus(e); Invalidate(); }
        protected override void OnLostFocus(EventArgs e) { base.OnLostFocus(e); Invalidate(); }

        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            e.Graphics.Clear(Parent == null ? BackColor : Parent.BackColor);
            bool highContrast = SystemInformation.HighContrast;
            Color text = highContrast ? SystemColors.ControlText : Enabled ? ForeColor : SystemColors.GrayText;
            TextRenderer.DrawText(e.Graphics, Text, Font, new Rectangle(0, 0, Width - 52, Height), text,
                TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis | TextFormatFlags.NoPrefix);
            Rectangle track = new Rectangle(Width - 44, (Height - 22) / 2, 42, 22);
            Color fill = highContrast ? (Checked ? SystemColors.Highlight : SystemColors.Control) :
                !Enabled ? Color.FromArgb(83, 88, 95) : Checked ? Color.FromArgb(0, 112, 205) : Color.FromArgb(91, 102, 113);
            using (System.Drawing.Drawing2D.GraphicsPath path = SettingsDrawing.Rounded(track, 11))
            using (Brush brush = new SolidBrush(fill))
            using (Pen border = new Pen(highContrast ? SystemColors.ControlText : fill))
            {
                e.Graphics.FillPath(brush, path);
                e.Graphics.DrawPath(border, path);
            }
            int knobX = Checked ? track.Right - 19 : track.Left + 3;
            using (Brush knob = new SolidBrush(highContrast ? SystemColors.ControlText : Color.White))
                e.Graphics.FillEllipse(knob, knobX, track.Top + 3, 16, 16);
            if (Focused && ShowFocusCues) ControlPaint.DrawFocusRectangle(e.Graphics, new Rectangle(0, 1, Width - 1, Height - 2));
        }
    }

    internal sealed class SettingsCardIcon : Control
    {
        private readonly string kind;

        public SettingsCardIcon(string kind)
        {
            this.kind = kind;
            Size = new Size(36, 36);
            AccessibleRole = AccessibleRole.None;
            SetStyle(ControlStyles.UserPaint | ControlStyles.OptimizedDoubleBuffer, true);
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            base.OnPaint(e);
            bool highContrast = SystemInformation.HighContrast;
            Color tile = highContrast ? SystemColors.Control : Color.FromArgb(39, 60, 80);
            Color line = highContrast ? SystemColors.ControlText : Color.FromArgb(105, 190, 250);
            e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            using (System.Drawing.Drawing2D.GraphicsPath path = SettingsDrawing.Rounded(new Rectangle(0, 0, Width - 1, Height - 1), 7))
            using (Brush background = new SolidBrush(tile)) e.Graphics.FillPath(background, path);
            using (Pen pen = new Pen(line, 2))
            {
                if (kind == "Tray icon display" || kind == "Tray background" || kind == "Tray preview")
                {
                    e.Graphics.DrawRectangle(pen, 8, 9, 20, 15);
                    e.Graphics.DrawLine(pen, 18, 24, 18, 28);
                    e.Graphics.DrawLine(pen, 13, 28, 23, 28);
                }
                else if (kind == "Usage threshold alerts")
                {
                    e.Graphics.DrawArc(pen, 11, 8, 14, 16, 180, 180);
                    e.Graphics.DrawLine(pen, 11, 16, 11, 24);
                    e.Graphics.DrawLine(pen, 25, 16, 25, 24);
                    e.Graphics.DrawLine(pen, 8, 24, 28, 24);
                    e.Graphics.DrawArc(pen, 15, 23, 6, 6, 0, 180);
                }
                else if (kind == "Auto-use resets" || kind == "Quiet hours" ||
                    kind == "Active refresh" || kind == "Idle refresh" || kind == "Updated time" || kind == "Reset times")
                {
                    e.Graphics.DrawEllipse(pen, 8, 8, 20, 20);
                    e.Graphics.DrawLine(pen, 18, 12, 18, 18);
                    e.Graphics.DrawLine(pen, 18, 18, 23, 21);
                }
                else if (kind == "Reset expiry reminders" || kind == "Application updates")
                {
                    e.Graphics.DrawArc(pen, 8, 8, 20, 20, 35, 275);
                    e.Graphics.DrawLine(pen, 24, 7, 28, 11);
                    e.Graphics.DrawLine(pen, 28, 11, 23, 12);
                }
                else if (kind == "Theme")
                {
                    e.Graphics.DrawEllipse(pen, 8, 8, 20, 20);
                    e.Graphics.DrawArc(pen, 13, 9, 14, 18, 90, 180);
                }
                else if (kind == "Tray text color")
                {
                    e.Graphics.DrawLine(pen, 9, 27, 17, 9);
                    e.Graphics.DrawLine(pen, 17, 9, 26, 27);
                    e.Graphics.DrawLine(pen, 12, 21, 23, 21);
                }
                else if (kind == "Startup")
                {
                    e.Graphics.DrawArc(pen, 9, 9, 18, 18, 45, 270);
                    e.Graphics.DrawLine(pen, 18, 6, 18, 18);
                }
                else
                {
                    e.Graphics.DrawLine(pen, 9, 27, 9, 20);
                    e.Graphics.DrawLine(pen, 16, 27, 16, 14);
                    e.Graphics.DrawLine(pen, 23, 27, 23, 9);
                    e.Graphics.DrawLine(pen, 7, 28, 28, 28);
                }
            }
        }
    }

    internal sealed class SettingsCard : Panel
    {
        public Color BorderColor { get; set; }

        public SettingsCard()
        {
            SetStyle(ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
        }

        protected override void OnResize(EventArgs e)
        {
            base.OnResize(e);
            if (Width <= 1 || Height <= 1) return;
            using (System.Drawing.Drawing2D.GraphicsPath path = SettingsDrawing.Rounded(new Rectangle(0, 0, Width, Height), 8))
            {
                Region old = Region;
                Region = new Region(path);
                if (old != null) old.Dispose();
            }
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            base.OnPaint(e);
            if (Width > 1 && Height > 1)
            {
                e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
                using (System.Drawing.Drawing2D.GraphicsPath path = SettingsDrawing.Rounded(new Rectangle(0, 0, Width - 1, Height - 1), 8))
                using (Pen pen = new Pen(BorderColor))
                    e.Graphics.DrawPath(pen, path);
            }
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
