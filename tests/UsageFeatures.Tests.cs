using System;
using System.IO;
using System.Reflection;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Windows.Forms;

namespace CodexUsageTray.Tests
{
    internal static class UsageFeaturesTests
    {
        [DllImport("user32.dll")]
        private static extern bool IsWindowVisible(IntPtr handle);

        [DllImport("user32.dll")]
        private static extern IntPtr SendMessage(IntPtr handle, int message, IntPtr wParam, IntPtr lParam);

        public static void Run()
        {
            TestTrayMetric();
            TestComboRendering();
            string directory = Path.Combine(Path.GetTempPath(), "CodexUsageFeatures-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(directory);
            try
            {
                TestReminders(Path.Combine(directory, "reminders.json"));
                TestSettings(Path.Combine(directory, "settings.json"));
                TestSettingsForm(Path.Combine(directory, "ui-settings.json"));
            }
            finally
            {
                foreach (string file in Directory.GetFiles(directory)) File.Delete(file);
                Directory.Delete(directory);
            }
        }

        private static void TestTrayMetric()
        {
            UsageSnapshot snapshot = new UsageSnapshot
            {
                Weekly = new LimitWindow { UsedPercent = 25 },
                FiveHour = new LimitWindow { UsedPercent = 80 }
            };
            Assert(TrayAppContext.GetIconWindow(snapshot, AppSettings.IconMetricAuto) == snapshot.FiveHour,
                "auto selects lower remaining percentage");
            Assert(TrayAppContext.GetIconWindow(snapshot, AppSettings.IconMetricWeekly) == snapshot.Weekly,
                "explicit weekly selection is preserved");
            snapshot.Weekly.UsedPercent = 90;
            Assert(TrayAppContext.GetIconWindow(snapshot, AppSettings.IconMetricAuto) == snapshot.Weekly,
                "auto switches when weekly becomes lower");
            snapshot.FiveHour.UsedPercent = 90;
            Assert(TrayAppContext.GetIconWindow(snapshot, AppSettings.IconMetricAuto) == snapshot.Weekly,
                "ties prefer weekly consistently");
            snapshot.Weekly = null;
            Assert(TrayAppContext.GetIconWindow(snapshot, AppSettings.IconMetricAuto) == snapshot.FiveHour,
                "auto handles an unavailable weekly window");
            Assert(TrayAppContext.GetIconWindow(snapshot, AppSettings.IconMetricWeekly) == snapshot.FiveHour,
                "explicit metric retains missing-window fallback");
            Assert(TrayAppContext.GetIconWindow(null, AppSettings.IconMetricAuto) == null, "no snapshot has no icon window");
        }

        private static void TestReminders(string path)
        {
            DateTime now = DateTime.UtcNow;
            UsageSnapshot snapshot = new UsageSnapshot { LastUpdated = now };
            snapshot.AvailableResets.Add(new RateLimitResetCredit { Id = "due", ExpiresAtUtc = now.AddHours(24) });
            snapshot.AvailableResets.Add(new RateLimitResetCredit { Id = "later", ExpiresAtUtc = now.AddHours(30) });
            snapshot.AvailableResets.Add(new RateLimitResetCredit { Id = "expired", ExpiresAtUtc = now });
            snapshot.AvailableResets.Add(new RateLimitResetCredit { Id = "unknown" });
            ResetExpiryReminder reminder = new ResetExpiryReminder(path);
            Assert(reminder.GetNextCheckUtc(snapshot, now, 24) == now, "lead-time boundary is due immediately");
            snapshot.IsStale = true;
            Assert(reminder.Observe(snapshot, now, 24).Count == 0, "stale data cannot trigger reminders");
            snapshot.IsStale = false;
            Assert(reminder.Observe(snapshot, now, 24).Count == 1, "only a known unexpired due credit notifies");
            Assert(reminder.Observe(snapshot, now, 24).Count == 0, "repeat polls do not notify twice");
            Assert(new ResetExpiryReminder(path).Observe(snapshot, now, 24).Count == 0, "deduplication survives restart");
            Assert(reminder.GetNextCheckUtc(snapshot, now, 24) == now.AddHours(6), "next check is scheduled before expiration");
            snapshot.AvailableResets.RemoveAt(1);
            Assert(reminder.GetNextCheckUtc(snapshot, now, 24) == null, "redeemed credit does not retain a scheduled reminder");
            snapshot.AvailableResets[0].ExpiresAtUtc = now.AddHours(23);
            Assert(reminder.Observe(snapshot, now, 24).Count == 1, "changed expiration permits an updated reminder");
            File.WriteAllText(path, "invalid");
            Assert(new ResetExpiryReminder(path).Observe(snapshot, now, 24).Count == 1, "corrupt cache does not break reminders");
        }

        private static void TestSettings(string path)
        {
            File.WriteAllText(path, "{\"SettingsVersion\":7,\"IconMetric\":\"FiveHour\",\"ShowResetAvailability\":false}");
            MethodInfo load = typeof(AppSettings).GetMethod("LoadFromPath", BindingFlags.NonPublic | BindingFlags.Static);
            MethodInfo save = typeof(AppSettings).GetMethod("SaveToPath", BindingFlags.NonPublic | BindingFlags.Instance);
            AppSettings settings = (AppSettings)load.Invoke(null, new object[] { path });
            Assert(settings.IconMetric == AppSettings.IconMetricFiveHour, "upgrade preserves chosen tray metric");
            Assert(settings.ResetExpiryReminders && settings.ResetExpiryLeadHours == 24, "upgrade initializes reminders");
            Assert(settings.ForecastMethod == AppSettings.ForecastLearnedRoutine && settings.QuietHoursMode == AppSettings.QuietHoursAutomatic &&
                settings.ShowProjectedUsage && !settings.HighlightQuietHours, "upgrade initializes forecast preferences");
            settings.IconMetric = AppSettings.IconMetricAuto;
            settings.ResetExpiryReminders = false;
            settings.ResetExpiryLeadHours = 6;
            settings.ForecastMethod = AppSettings.ForecastRecentPace;
            settings.QuietHoursMode = AppSettings.QuietHoursOff;
            settings.ShowProjectedUsage = false;
            settings.HighlightQuietHours = true;
            save.Invoke(settings, new object[] { path });
            AppSettings reloaded = (AppSettings)load.Invoke(null, new object[] { path });
            Assert(reloaded.IconMetric == AppSettings.IconMetricAuto && !reloaded.ResetExpiryReminders &&
                reloaded.ResetExpiryLeadHours == 6 && reloaded.ForecastMethod == AppSettings.ForecastRecentPace &&
                reloaded.QuietHoursMode == AppSettings.QuietHoursOff && !reloaded.ShowProjectedUsage && reloaded.HighlightQuietHours,
                "new settings round trip");
            AppSettings copy = new AppSettings();
            copy.CopyValuesFrom(reloaded);
            Assert(copy.IconMetric == AppSettings.IconMetricAuto && !copy.ResetExpiryReminders &&
                copy.ResetExpiryLeadHours == 6 && copy.ForecastMethod == AppSettings.ForecastRecentPace &&
                copy.QuietHoursMode == AppSettings.QuietHoursOff && !copy.ShowProjectedUsage && copy.HighlightQuietHours,
                "applying settings copies new values");
        }

        private static void TestSettingsForm(string path)
        {
            MethodInfo save = typeof(AppSettings).GetMethod("SaveToPath", BindingFlags.NonPublic | BindingFlags.Instance);
            MethodInfo load = typeof(AppSettings).GetMethod("LoadFromPath", BindingFlags.NonPublic | BindingFlags.Static);
            AppSettings settings = new AppSettings { IconMetric = AppSettings.IconMetricAuto, LowThreshold = 35 };
            int saves = 0;
            using (SettingsForm form = new SettingsForm(settings, delegate(AppSettings candidate)
            {
                saves++;
                save.Invoke(candidate, new object[] { path });
            }))
            {
                Color initialBackground = form.BackColor;
                form.BackColor = Color.Magenta;
                typeof(SettingsForm).GetMethod("OnSystemColorsChanged", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(form, new object[] { EventArgs.Empty });
                Assert(form.BackColor == initialBackground && saves == 0,
                    "system color changes reapply the selected settings theme without saving");
                PrepareHiddenForm(form);
                Application.DoEvents();
                Assert(!form.HasUnsavedChanges && !TrayAppContext.ShouldWaitForSettings(form, true) &&
                    TrayAppContext.ShouldWaitForSettings(form, false),
                    "a manual update can restart with unchanged settings while a background update waits");
                NumericUpDown sample = (NumericUpDown)Field(form, "previewPercentNumeric");
                decimal preview = sample.Value;
                sample.Value = 17;
                Assert(!form.HasUnsavedChanges, "changing the preview alone does not create an unsaved setting");
                sample.Value = preview;
                NumericUpDown pendingNumber = (NumericUpDown)Field(form, "criticalNumeric");
                string originalNumberText = pendingNumber.Text;
                pendingNumber.Text = originalNumberText == "18" ? "19" : "18";
                Assert(form.HasUnsavedChanges, "uncommitted numeric text prevents a manual restart");
                pendingNumber.Text = originalNumberText;
                Assert(!form.HasUnsavedChanges, "reverting uncommitted numeric text clears the draft");
                CheckBox popupTimes = (CheckBox)Field(form, "showResetTimesCheckBox");
                bool originalTimes = popupTimes.Checked;
                popupTimes.Checked = !originalTimes;
                Assert(form.HasUnsavedChanges && TrayAppContext.ShouldWaitForSettings(form, true),
                    "a manual update waits for an edited setting");
                popupTimes.Checked = originalTimes;
                Assert(!form.HasUnsavedChanges, "reverting an edit permits the update to continue");
                FieldInfo colorField = typeof(SettingsForm).GetField("trayTextColor", BindingFlags.NonPublic | BindingFlags.Instance);
                Color originalTextColor = (Color)colorField.GetValue(form);
                colorField.SetValue(form, Color.Magenta);
                Assert(form.HasUnsavedChanges, "custom color changes are protected");
                colorField.SetValue(form, originalTextColor);
                FieldInfo choosingColor = typeof(SettingsForm).GetField("choosingColor", BindingFlags.NonPublic | BindingFlags.Instance);
                choosingColor.SetValue(form, true);
                Assert(form.HasUnsavedChanges, "an open color picker postpones restart");
                choosingColor.SetValue(form, false);
                Assert(!form.HasUnsavedChanges, "canceling an unchanged color picker permits restart");
                Button[] navigation = (Button[])Field(form, "navigationButtons");
                Control[] pages = (Control[])Field(form, "pages");
                for (int i = 0; i < navigation.Length; i++)
                {
                    navigation[i].PerformClick();
                    for (int j = 0; j < pages.Length; j++)
                        Assert(pages[j].Visible == (i == j), "settings navigation shows only its selected page");
                    int[] minimumCards = { 3, 4, 3, 4, 3, 1 };
                    Assert(CountCards(pages[i]) >= minimumCards[i], "every settings page uses grouped cards");
                    Assert(!IsWindowVisible(form.Handle), "settings navigation must not show a preview window");
                }
                navigation[1].PerformClick();
                CheckBox alerts = (CheckBox)Field(form, "thresholdNotificationsCheckBox");
                Control thresholdControls = (Control)Field(form, "thresholdControls");
                Assert(!thresholdControls.Enabled, "threshold settings are disabled when alerts are off");
                alerts.Checked = true;
                Assert(thresholdControls.Enabled, "threshold settings enable with alerts");
                NumericUpDown critical = (NumericUpDown)Field(form, "criticalNumeric");
                NumericUpDown low = (NumericUpDown)Field(form, "lowNumeric");
                critical.Value = 45;
                Assert(low.Value == 45, "critical threshold keeps low threshold valid");

                navigation[2].PerformClick();
                RadioButton recent = (RadioButton)Field(form, "recentPaceRadio");
                RadioButton learned = (RadioButton)Field(form, "learnedRoutineRadio");
                ComboBox quiet = (ComboBox)Field(form, "quietHoursCombo");
                CheckBox projected = (CheckBox)Field(form, "showProjectedUsageCheckBox");
                CheckBox highlight = (CheckBox)Field(form, "highlightQuietHoursCheckBox");
                recent.Checked = true;
                Assert(!quiet.Enabled && !highlight.Enabled, "recent pace disables learned quiet-hour controls");
                learned.Checked = true;
                quiet.SelectedIndex = 1;
                Assert(quiet.Enabled && !highlight.Enabled, "turning off learned quiet hours disables highlighting");
                quiet.SelectedIndex = 0;
                projected.Checked = true;
                Assert(highlight.Enabled, "quiet-hour highlighting can be enabled for the learned forecast");

                navigation[3].PerformClick();
                ComboBox theme = (ComboBox)Field(form, "themeCombo");
                Assert(theme.SelectedIndex >= 0 && !string.IsNullOrEmpty(theme.Text), "theme shows a selected value");
                theme.SelectedIndex = 0;
                SendMessage(theme.Handle, 0x100, new IntPtr(0x28), IntPtr.Zero);
                SendMessage(theme.Handle, 0x101, new IntPtr(0x28), IntPtr.Zero);
                Assert(theme.SelectedIndex == 1, "theme selector remains keyboard navigable");
                CheckBox trayBackground = (CheckBox)Field(form, "showTrayBoxCheckBox");
                Control trayBackgroundControls = (Control)Field(form, "trayBackgroundControls");
                trayBackground.Checked = false;
                Assert(!trayBackgroundControls.Enabled, "background color controls disable when the background is hidden");
                trayBackground.Checked = true;
                Assert(trayBackgroundControls.Enabled, "background color controls enable when the background is shown");
                ((NumericUpDown)Field(form, "previewPercentNumeric")).Value = 17;
                Assert(((PictureBox)Field(form, "trayPreview")).Image != null, "tray preview updates at the chosen sample value");

                typeof(SettingsForm).GetMethod("RestoreDefaults", BindingFlags.NonPublic | BindingFlags.Instance).Invoke(form, null);
                Assert(settings.IconMetric == AppSettings.IconMetricAuto && settings.LowThreshold == 35 && saves == 0,
                    "restore defaults changes controls without saving settings");
                recent.Checked = true;
                projected.Checked = false;
                bool applied = (bool)typeof(SettingsForm).GetMethod("TryApplyToSettings", BindingFlags.NonPublic | BindingFlags.Instance).Invoke(form, null);
                AppSettings persisted = (AppSettings)load.Invoke(null, new object[] { path });
                Assert(applied && saves == 1 && settings.ForecastMethod == AppSettings.ForecastRecentPace &&
                    !settings.ShowProjectedUsage && persisted.ForecastMethod == AppSettings.ForecastRecentPace && !persisted.ShowProjectedUsage,
                    "save persists and applies forecast controls");
                Assert(!form.HasUnsavedChanges, "a successful save updates the draft baseline");
                form.Close();
            }
            using (SettingsForm form = new SettingsForm(settings, delegate(AppSettings candidate) { saves++; }))
            {
                PrepareHiddenForm(form);
                ((RadioButton)Field(form, "learnedRoutineRadio")).Checked = true;
                ((Button)form.CancelButton).PerformClick();
                Assert((form.IsDisposed || form.DialogResult == DialogResult.Cancel) && saves == 1 &&
                    settings.ForecastMethod == AppSettings.ForecastRecentPace,
                    "cancel discards unsaved controls");
            }
        }

        private static void PrepareHiddenForm(Form form)
        {
            IntPtr handle = form.Handle;
            // Make child controls participate in layout and clicks without ever
            // setting the native top-level window visible or activating it.
            typeof(Control).GetMethod("SetState", BindingFlags.NonPublic | BindingFlags.Instance)
                .Invoke(form, new object[] { 2, true });
            form.CreateControl();
            Assert(!IsWindowVisible(handle), "settings tests must not show a preview window");
        }

        private static int CountCards(Control root)
        {
            int count = root is SettingsCard ? 1 : 0;
            foreach (Control child in root.Controls) count += CountCards(child);
            return count;
        }

        private static void TestComboRendering()
        {
            using (ComboBox combo = new ComboBox())
            {
                combo.Items.AddRange(new object[] { "System Default", "Dark Mode", "Light Mode" });
                combo.SelectedIndex = 1;
                foreach (bool highContrast in new[] { false, true })
                foreach (bool selected in new[] { false, true })
                using (Bitmap bitmap = new Bitmap(210, 32))
                using (Graphics graphics = Graphics.FromImage(bitmap))
                {
                    combo.BackColor = highContrast ? SystemColors.Window : Color.FromArgb(31, 36, 43);
                    combo.ForeColor = highContrast ? SystemColors.WindowText : Color.White;
                    Color expected = selected ? (highContrast ? SystemColors.Highlight : Color.FromArgb(31, 68, 105)) : combo.BackColor;
                    DrawItemState state = selected ? DrawItemState.Selected : DrawItemState.ComboBoxEdit;
                    DrawItemEventArgs args = new DrawItemEventArgs(graphics, combo.Font,
                        new Rectangle(0, 0, bitmap.Width, bitmap.Height), combo.SelectedIndex, state, combo.ForeColor, combo.BackColor);
                    SettingsForm.DrawComboItem(combo, args, highContrast);
                    Assert(bitmap.GetPixel(0, 0).ToArgb() == expected.ToArgb(), "combo uses the correct theme background");
                    int textPixels = 0;
                    for (int y = 2; y < bitmap.Height - 2; y++)
                        for (int x = 3; x < bitmap.Width - 3; x++)
                            if (bitmap.GetPixel(x, y).ToArgb() != expected.ToArgb()) textPixels++;
                    Assert(textPixels > 10, "closed and highlighted combo selections render readable text");
                }
            }
        }

        private static object Field(object instance, string name)
        {
            return instance.GetType().GetField(name, BindingFlags.NonPublic | BindingFlags.Instance).GetValue(instance);
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException(message);
        }
    }
}
