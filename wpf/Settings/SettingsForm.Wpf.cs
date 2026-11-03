using System;
using System.IO;
using System.Windows.Forms.Integration;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace CodexUsageTray
{
    internal sealed partial class SettingsForm
    {
        private WpfSettingsView wpfSettings;
        private ElementHost wpfHost;
        private System.Windows.Forms.Control legacySettingsRoot;

        private void InitializeWpfSettings()
        {
            // Keep Windows' high-contrast UI on the native, system-color-aware path.
            if (System.Windows.Forms.SystemInformation.HighContrast) return;
            legacySettingsRoot = Controls.Count == 0 ? null : Controls[0];
            wpfSettings = new WpfSettingsView(this);
            wpfHost = new ElementHost { Dock = System.Windows.Forms.DockStyle.Fill, Child = wpfSettings };
            Controls.Add(wpfHost);
            if (legacySettingsRoot != null) legacySettingsRoot.Visible = false;
            wpfHost.BringToFront();
            ClientSize = new System.Drawing.Size(1200, 732);
            MinimumSize = new System.Drawing.Size(980, 650);
            SynchronizeWpfSettings();
        }

        private void UpdateWpfSystemColors()
        {
            if (wpfHost == null) return;
            wpfHost.Visible = !System.Windows.Forms.SystemInformation.HighContrast;
            if (legacySettingsRoot != null) legacySettingsRoot.Visible = !wpfHost.Visible;
            if (wpfHost.Visible) SynchronizeWpfSettings();
        }

        internal void WpfInputChanged(object source)
        {
            if (wpfSettings == null || wpfSettings.Synchronizing) return;
            var view = wpfSettings;
            if (source == view.MetricWeekly && view.MetricWeekly.IsChecked == true) weeklyRadio.Checked = true;
            else if (source == view.MetricFiveHour && view.MetricFiveHour.IsChecked == true) fiveHourRadio.Checked = true;
            else if (source == view.MetricAuto && view.MetricAuto.IsChecked == true) autoMetricRadio.Checked = true;
            else if (source == view.CriticalValue) SetWpfNumeric(criticalNumeric, view.CriticalValue.Value);
            else if (source == view.LowValue) SetWpfNumeric(lowNumeric, view.LowValue.Value);
            else if (source == view.ThresholdAlerts) thresholdNotificationsCheckBox.Checked = view.ThresholdAlerts.IsChecked == true;
            else if (source == view.ReminderEnabled) resetExpiryRemindersCheckBox.Checked = view.ReminderEnabled.IsChecked == true;
            else if (source == view.ReminderHours) SetWpfNumeric(resetExpiryLeadNumeric, view.ReminderHours.Value);
            else if (source == view.AutoUseEnabled) autoRedeemResetCreditsCheckBox.Checked = view.AutoUseEnabled.IsChecked == true;
            else if (source == view.AutoUseMinutes) SetWpfNumeric(autoRedeemLeadNumeric, view.AutoUseMinutes.Value);
            else if (source == view.ForecastLearned && view.ForecastLearned.IsChecked == true) learnedRoutineRadio.Checked = true;
            else if (source == view.ForecastRecent && view.ForecastRecent.IsChecked == true) recentPaceRadio.Checked = true;
            else if (source == view.QuietHours && view.QuietHours.SelectedIndex >= 0) quietHoursCombo.SelectedIndex = view.QuietHours.SelectedIndex;
            else if (source == view.ShowProjection) showProjectedUsageCheckBox.Checked = view.ShowProjection.IsChecked == true;
            else if (source == view.HighlightQuiet) highlightQuietHoursCheckBox.Checked = view.HighlightQuiet.IsChecked == true;
            else if (source == view.RunningRefresh) SetWpfNumeric(refreshNumeric, view.RunningRefresh.Value);
            else if (source == view.IdleRefresh) SetWpfNumeric(idleRefreshNumeric, view.IdleRefresh.Value);
            else if (source == view.StartWithWindows) startWithWindowsCheckBox.Checked = view.StartWithWindows.IsChecked == true;
            else if (source == view.ThemeChoice && view.ThemeChoice.SelectedIndex >= 0) themeCombo.SelectedIndex = view.ThemeChoice.SelectedIndex;
            else if (source == view.TrayBackground) showTrayBoxCheckBox.Checked = view.TrayBackground.IsChecked == true;
            else if (source == view.PreviewRemaining) SetWpfNumeric(previewPercentNumeric, view.PreviewRemaining.Value);
            else if (source == view.ShowUpdatedTime) showLastUpdatedCheckBox.Checked = view.ShowUpdatedTime.IsChecked == true;
            else if (source == view.ShowResetTimes) showResetTimesCheckBox.Checked = view.ShowResetTimes.IsChecked == true;
            else if (source == view.ColorUsageBars) colorBarsCheckBox.Checked = view.ColorUsageBars.IsChecked == true;
            else return;
            UpdateDependentControlStates();
            UpdatePreviews();
            SynchronizeWpfSettings();
        }

        private static void SetWpfNumeric(System.Windows.Forms.NumericUpDown control, double? value)
        {
            if (!value.HasValue || double.IsNaN(value.Value) || double.IsInfinity(value.Value)) return;
            decimal rounded = (decimal)Math.Round(value.Value);
            control.Value = Math.Max(control.Minimum, Math.Min(control.Maximum, rounded));
        }

        private void SynchronizeWpfSettings()
        {
            if (wpfSettings == null) return;
            var view = wpfSettings;
            view.Synchronizing = true;
            try
            {
                view.MetricWeekly.IsChecked = weeklyRadio.Checked;
                view.MetricFiveHour.IsChecked = fiveHourRadio.Checked;
                view.MetricAuto.IsChecked = autoMetricRadio.Checked;
                view.CriticalValue.Value = (double)criticalNumeric.Value;
                view.LowValue.Value = (double)lowNumeric.Value;
                view.ThresholdAlerts.IsChecked = thresholdNotificationsCheckBox.Checked;
                view.ReminderEnabled.IsChecked = resetExpiryRemindersCheckBox.Checked;
                view.ReminderHours.Value = (double)resetExpiryLeadNumeric.Value;
                view.AutoUseEnabled.IsChecked = autoRedeemResetCreditsCheckBox.Checked;
                view.AutoUseMinutes.Value = (double)autoRedeemLeadNumeric.Value;
                view.ForecastLearned.IsChecked = learnedRoutineRadio.Checked;
                view.ForecastRecent.IsChecked = recentPaceRadio.Checked;
                view.QuietHours.SelectedIndex = quietHoursCombo.SelectedIndex;
                view.QuietHours.IsEnabled = quietHoursCombo.Enabled;
                view.ShowProjection.IsChecked = showProjectedUsageCheckBox.Checked;
                view.HighlightQuiet.IsChecked = highlightQuietHoursCheckBox.Checked;
                view.HighlightQuiet.IsEnabled = highlightQuietHoursCheckBox.Enabled;
                view.RunningRefresh.Value = (double)refreshNumeric.Value;
                view.IdleRefresh.Value = (double)idleRefreshNumeric.Value;
                view.StartWithWindows.IsChecked = startWithWindowsCheckBox.Checked;
                view.ThemeChoice.SelectedIndex = themeCombo.SelectedIndex;
                view.ApplyTheme(AppSettings.IsDarkTheme(GetSelectedTheme()));
                view.TrayBackground.IsChecked = showTrayBoxCheckBox.Checked;
                view.PreviewRemaining.Value = (double)previewPercentNumeric.Value;
                view.ShowUpdatedTime.IsChecked = showLastUpdatedCheckBox.Checked;
                view.ShowResetTimes.IsChecked = showResetTimesCheckBox.Checked;
                view.ColorUsageBars.IsChecked = colorBarsCheckBox.Checked;
                view.TrayBoxColor.Background = WpfBrush(trayBoxColorButton.BackColor);
                view.TrayTextColor.Background = WpfBrush(trayTextColorButton.BackColor);
                view.TrayBoxHex.Text = trayBoxHexLabel.Text;
                view.TrayTextHex.Text = trayTextHexLabel.Text;
                view.TrayBoxAuto.IsEnabled = trayBoxAutoButton.Enabled;
                view.ContrastStatus.Text = contrastStatusLabel.Text;
                view.VersionText.Text = "Version " + AppVersion.Current;
                if (trayPreview.Image != null)
                {
                    using (var stream = new MemoryStream())
                    {
                        trayPreview.Image.Save(stream, System.Drawing.Imaging.ImageFormat.Png);
                        stream.Position = 0;
                        var image = new BitmapImage();
                        image.BeginInit();
                        image.CacheOption = BitmapCacheOption.OnLoad;
                        image.StreamSource = stream;
                        image.EndInit();
                        image.Freeze();
                        view.TrayPreview.Source = image;
                    }
                }
                RefreshWpfSettingsStatus();
            }
            finally { view.Synchronizing = false; }
        }

        private static SolidColorBrush WpfBrush(System.Drawing.Color color)
        {
            return new SolidColorBrush(System.Windows.Media.Color.FromArgb(color.A, color.R, color.G, color.B));
        }

        private void RefreshWpfSettingsStatus()
        {
            if (wpfSettings == null) return;
            wpfSettings.CheckUpdates.IsEnabled = checkUpdatesButton.Enabled;
            wpfSettings.CheckUpdates.Content = checkUpdatesButton.Text.Replace("&", "");
            wpfSettings.UpdateStatus.Text = updateStatusLabel.Text;
        }

        internal void WpfRestoreDefaults() { RestoreDefaults(); SynchronizeWpfSettings(); }
        internal void WpfSave() { if (TryApplyToSettings()) Close(); }
        internal void WpfPickTrayBoxColor() { PickTrayBoxColor(); SynchronizeWpfSettings(); }
        internal void WpfPickTrayTextColor() { PickTrayTextColor(); SynchronizeWpfSettings(); }
        internal void WpfUseAutomaticTrayColor() { useCustomTrayBoxColor = false; UpdatePreviews(); SynchronizeWpfSettings(); }
        internal void WpfCheckUpdates() { if (CheckUpdatesRequested != null) CheckUpdatesRequested(this, EventArgs.Empty); }
    }
}
