using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.Drawing.Imaging;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Forms.Integration;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using Forms = System.Windows.Forms;

namespace CodexUsageTray
{
    internal static class IntegrationCheck
    {
        [STAThread]
        private static int Main(string[] args)
        {
            if (args.Length == 1)
            {
                string output = PrepareOutput(args[0]);
                return PrivateDesktop.Launch(Assembly.GetExecutingAssembly().Location, output);
            }
            if (args.Length == 4 && args[0] == "--legacy")
            {
                string tests = Path.GetFullPath(args[1]);
                string fixture = Path.GetFullPath(args[2]);
                if (!File.Exists(tests) || !File.Exists(fixture) || tests.IndexOf('"') >= 0 || fixture.IndexOf('"') >= 0)
                    throw new ArgumentException("Legacy executable and fixture must exist and not contain quotes.");
                string output = PrepareOutput(args[3]);
                return PrivateDesktop.LaunchLegacy(Assembly.GetExecutingAssembly().Location, tests, fixture, output);
            }
            if (args.Length == 4 && args[0] == "--legacy-child")
                return RunLegacyOnPrivateDesktop(args[1], args[2], args[3]);
            if (args.Length != 2 || args[0] != "--capture") return 2;
            try
            {
                // Guard before constructing any form or loading any UI resources.
                PrivateDesktop.RequirePrivateDesktop();
                RunChecks(Path.GetFullPath(args[1]));
                return 0;
            }
            catch (Exception error)
            {
                File.WriteAllText(Path.Combine(args[1], "error.txt"), error.ToString());
                return 1;
            }
        }

        private static string PrepareOutput(string path)
        {
            string output = Path.GetFullPath(path);
            if (output.IndexOf('"') >= 0) throw new ArgumentException("Output path cannot contain a quote.");
            Directory.CreateDirectory(output);
            if (Directory.GetFileSystemEntries(output).Length != 0)
                throw new InvalidOperationException("Choose an empty output directory for every run.");
            return output;
        }

        private static int RunLegacyOnPrivateDesktop(string tests, string fixture, string output)
        {
            try
            {
                PrivateDesktop.RequirePrivateDesktop();
                string isolated = Path.Combine(output, "sample-data");
                Directory.CreateDirectory(isolated);
                Environment.SetEnvironmentVariable("APPDATA", isolated);
                Environment.SetEnvironmentVariable("LOCALAPPDATA", isolated);
                Environment.SetEnvironmentVariable("TEMP", isolated);
                Environment.SetEnvironmentVariable("TMP", isolated);
                var log = new StringWriter();
                Console.SetOut(log);
                Console.SetError(log);
                int exitCode;
                try
                {
                    MethodInfo entry = Assembly.LoadFrom(tests).EntryPoint;
                    if (entry == null) throw new InvalidOperationException("Legacy test assembly has no entry point.");
                    object result = entry.Invoke(null, new object[] { new string[] { fixture } });
                    exitCode = result is int ? (int)result : 0;
                }
                catch (TargetInvocationException error)
                {
                    log.WriteLine(error.InnerException ?? error);
                    exitCode = 1;
                }
                File.WriteAllText(Path.Combine(output, "legacy-log.txt"), log.ToString());
                return exitCode;
            }
            catch (Exception error)
            {
                File.WriteAllText(Path.Combine(output, "error.txt"), error.ToString());
                return 1;
            }
        }

        private static void RunChecks(string output)
        {
            string sampleDir = Path.Combine(output, "sample-data");
            Directory.CreateDirectory(sampleDir);
            // Never construct the tray context or use a default history/settings path.
            Environment.SetEnvironmentVariable("APPDATA", sampleDir);
            Environment.SetEnvironmentVariable("LOCALAPPDATA", sampleDir);
            Environment.SetEnvironmentVariable("TEMP", sampleDir);
            Environment.SetEnvironmentVariable("TMP", sampleDir);
            Forms.Application.EnableVisualStyles();
            Forms.Application.SetCompatibleTextRenderingDefault(false);
            WpfBootstrap.Initialize();

            var settings = new AppSettings { Theme = AppSettings.ThemeDark };
            var history = new UsageHistoryStore(Path.Combine(sampleDir, "history.json"), settings);
            DateTime now = DateTime.UtcNow.AddSeconds(-10);
            var readings = new List<UsageSnapshot>();
            for (int hours = 144; hours >= 0; hours--)
            {
                DateTime timestamp = now.AddHours(-hours);
                readings.Add(new UsageSnapshot
                {
                    LastUpdated = timestamp,
                    Weekly = new LimitWindow { UsedPercent = 28.0 - hours / 6.0, WindowMinutes = 10080,
                        ResetAfterSeconds = (int)(TimeSpan.FromDays(2).TotalSeconds + hours * 3600) },
                    FiveHour = new LimitWindow { UsedPercent = 35, WindowMinutes = 300, ResetAfterSeconds = 7200 }
                });
            }
            history.Import(readings, true, false);
            var snapshot = readings[readings.Count - 1].Clone();
            snapshot.AvailableResetCount = 2;
            snapshot.AvailableResets.Add(new RateLimitResetCredit { Title = "Full reset", ExpiresAtUtc = now.AddDays(1) });
            snapshot.AvailableResets.Add(new RateLimitResetCredit { Title = "Weekly reset", ExpiresAtUtc = now.AddDays(3) });

            var failures = new List<string>();
            RunPart("Popup", failures, delegate
            {
            using (var popup = new UsagePopup(settings, history))
            {
                int refreshes = 0, historyRequests = 0, settingsRequests = 0;
                popup.RefreshRequested += delegate { refreshes++; };
                popup.HistoryRequested += delegate { historyRequests++; };
                popup.SettingsRequested += delegate { settingsRequests++; };
                popup.UpdateSnapshot(snapshot);
                popup.ShowNear(new System.Drawing.Point(1000, 750));
                Pump();
                CaptureForm(popup, output, "popup");
                var host = RequireHost(popup, "popup");
                CaptureWpf(host, output, "popup-wpf");
                var view = (FrameworkElement)host.Child;
                Check(view.ActualWidth > 100 && view.ActualHeight > 100, "Popup WPF surface has a usable layout");
                Check(HasText(view, "72%") && HasText(view, "65%") &&
                    !HasText(view, "72% remaining") && !HasText(view, "65% remaining"),
                    "Popup shows the sample percentages once, in the bars");
                Check(HasAutomationName(view, "Weekly limit, 72 percent remaining") &&
                    HasAutomationName(view, "5-hour limit, 65 percent remaining"), "Popup bars retain accessible values");
                Check(!HasText(view, "Avg. remaining") && !HasText(view, "Observed avg."), "Popup does not display an average");
                var dailyGrid = FindVisual<UniformGrid>(view);
                Check(dailyGrid != null && dailyGrid.Children.Count == 7, "Popup exposes seven daily usage bars");
                var actualDaily = UsagePopup.BuildWeeklyHistorySummary(history.Samples, DateTime.UtcNow);
                for (int day = 0; day < 7; day++)
                {
                    var dayButton = dailyGrid.Children[day] as System.Windows.Controls.Button;
                    Check(dayButton != null && dayButton.ActualHeight >= 24,
                        "Daily bar has a full-height hover target; actual=" + (dayButton == null ? "null" : dayButton.ActualWidth + "x" + dayButton.ActualHeight) +
                        ", grid=" + dailyGrid.ActualWidth + "x" + dailyGrid.ActualHeight +
                        ", alignment=" + dayButton.HorizontalAlignment + "/" + dayButton.VerticalAlignment + ", margin=" + dayButton.Margin +
                        ", grid columns=" + dailyGrid.Columns + ", rows=" + dailyGrid.Rows + ", children=" + dailyGrid.Children.Count);
                    var tip = dayButton.ToolTip as ToolTip;
                    var content = tip == null ? null : tip.Content as TextBlock;
                    Check(content != null && ToolTipService.GetInitialShowDelay(dayButton) == 0 &&
                        ToolTipService.GetBetweenShowDelay(dayButton) == 0, "Daily tooltip appears without a hover delay");
                    string expected = actualDaily.DailyUsage[day].HasValue
                        ? "Observed: " + actualDaily.DailyUsage[day].Value.ToString("0.###", CultureInfo.CurrentCulture) + "% used"
                        : "Usage unavailable";
                    Check(content.Text.Contains(expected) && content.Text.Contains(actualDaily.DailyDates[day].ToString("ddd, MMM d", CultureInfo.CurrentCulture)) &&
                        AutomationProperties.GetName(dayButton) == content.Text.Replace("\n", "; "),
                        "Daily bar exposes its real observed usage and date accessibly");
                    if (actualDaily.DailyUsage[day].HasValue && actualDaily.DailyPartial[day])
                        Check(content.Text.Contains("Partial coverage"), "Daily tooltip explains partial coverage");
                }
                var hoverButton = (System.Windows.Controls.Button)dailyGrid.Children[5];
                var hit = (Grid)hoverButton.Content;
                var dailyBar = (Border)hit.Children[0];
                System.Windows.Media.Color beforeHover = ((SolidColorBrush)dailyBar.Background).Color;
                hoverButton.RaiseEvent(new MouseEventArgs(Mouse.PrimaryDevice, 0) { RoutedEvent = Mouse.MouseEnterEvent });
                Check(((SolidColorBrush)dailyBar.Background).Color != beforeHover, "Daily bar immediately highlights on mouse enter");
                CaptureWpf(host, output, "popup-day-hover-wpf");
                var openedTip = (ToolTip)hoverButton.ToolTip;
                openedTip.PlacementTarget = hoverButton;
                openedTip.IsOpen = true;
                Pump();
                var openedContent = (TextBlock)openedTip.Content;
                System.Windows.Point textOrigin = openedContent.TranslatePoint(new System.Windows.Point(0, 0), openedTip);
                File.WriteAllText(Path.Combine(output, "tooltip-layout.txt"), "Tooltip actual=" + openedTip.ActualWidth + "x" + openedTip.ActualHeight +
                    "; content origin=" + textOrigin + "; content actual=" + openedContent.ActualWidth + "x" + openedContent.ActualHeight +
                    "; content desired=" + openedContent.DesiredSize + "; tooltip render=" + openedTip.RenderSize + ".\n");
                Check(openedTip.IsOpen && openedTip.IsVisible && textOrigin.X >= 0 && textOrigin.Y >= 0 &&
                    textOrigin.X + openedContent.DesiredSize.Width <= openedTip.ActualWidth &&
                    textOrigin.Y + openedContent.DesiredSize.Height <= openedTip.ActualHeight && openedContent.Text.Contains("Observed:") &&
                    openedContent.Text.Contains("% used") && openedContent.Text.Contains("Partial coverage"),
                    "Opened day tooltip shows unclipped date, observed usage and coverage");
                var tipSource = PresentationSource.FromVisual(openedTip) as HwndSource;
                NativeRect tipBounds = new NativeRect();
                Check(tipSource != null && GetWindowRect(tipSource.Handle, out tipBounds), "Opened tooltip has a native window");
                var workArea = Forms.Screen.FromHandle(tipSource.Handle).WorkingArea;
                Check(tipBounds.Left >= workArea.Left && tipBounds.Top >= workArea.Top &&
                    tipBounds.Right <= workArea.Right && tipBounds.Bottom <= workArea.Bottom,
                    "Opened tooltip is positioned inside the working area");
                CaptureVisual(openedTip, output, "popup-day-tooltip-wpf");
                openedTip.IsOpen = false;
                Pump();
                hoverButton.RaiseEvent(new MouseEventArgs(Mouse.PrimaryDevice, 0) { RoutedEvent = Mouse.MouseLeaveEvent });
                Check(((SolidColorBrush)dailyBar.Background).Color == beforeHover, "Daily bar returns to its normal color on mouse leave");
                CheckDailyTooltipCases();
                var tooltipField = typeof(UsagePopup).GetField("actionToolTip", BindingFlags.Instance | BindingFlags.NonPublic);
                var tooltips = (Forms.ToolTip)tooltipField.GetValue(popup);
                Check(string.IsNullOrEmpty(tooltips.GetToolTip(popup)) && popup.AccessibleDescription.Contains("Full reset"),
                    "No popup-wide tooltip; reset details remain accessible");
                var refresh = FindAction(view, "Refresh usage");
                var historyAction = FindAction(view, "Open usage history");
                var settingsAction = FindAction(view, "Open settings");
                Check(refresh != null && historyAction != null && settingsAction != null, "Popup actions are exposed");
                refresh.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                historyAction.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                settingsAction.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                Check(refreshes == 1 && historyRequests == 1 && settingsRequests == 1, "Popup actions reach the real host events");
                FocusManager.SetIsFocusScope(view, true);
                FocusManager.SetFocusedElement(view, historyAction);
                popup.UpdateSnapshot(snapshot.Clone());
                Pump();
                Check(ReferenceEquals(historyAction, FindAction(view, "Open usage history")) &&
                    ReferenceEquals(FocusManager.GetFocusedElement(view), historyAction), "Refresh preserves the focused history control");
                popup.SetRefreshing(true);
                Pump();
                var refreshing = FindAction(view, "Refreshing usage");
                Check(refreshing != null && !refreshing.IsEnabled, "Refreshing prevents duplicate requests");
                popup.SetRefreshing(false);
                popup.UpdateClaudeSnapshot(snapshot.Clone(), history);
                Pump();
                var switchToClaude = FindAction(view, "Showing Codex usage; switch provider");
                Check(switchToClaude != null, "Provider switch is available");
                switchToClaude.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                Check(popup.IsShowingClaude, "Provider switch opens Claude usage");
                var switchToCodex = FindAction(view, "Showing Claude usage; switch provider");
                Check(switchToCodex != null, "Provider switch remains available");
                switchToCodex.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                Check(!popup.IsShowingClaude, "Provider switch restores Codex usage");
                var narrowLayout = typeof(UsagePopup).GetMethod("ApplyLayoutMetricsInArea", BindingFlags.Instance | BindingFlags.NonPublic);
                Check(narrowLayout != null, "Popup supports area-constrained layout");
                narrowLayout.Invoke(popup, new object[] { false, 650, 420 });
                Pump();
                CaptureForm(popup, output, "popup-narrow");
                CaptureWpf(host, output, "popup-narrow-wpf");
                var manyResets = snapshot.Clone();
                for (int i = 0; i < 35; i++)
                    manyResets.AvailableResets.Add(new RateLimitResetCredit { Title = "Extra reset " + i, ExpiresAtUtc = now.AddDays(5) });
                manyResets.AvailableResetCount = manyResets.AvailableResets.Count;
                popup.UpdateSnapshot(manyResets);
                Pump();
                var scrollField = typeof(UsagePopupView).GetField("scroll", BindingFlags.Instance | BindingFlags.NonPublic);
                var scroll = (ScrollViewer)scrollField.GetValue(view);
                Check(scroll.ScrollableHeight > 100, "Long reset list is scrollable");
                scroll.ScrollToVerticalOffset(100);
                Pump();
                double offset = scroll.VerticalOffset;
                Check(offset > 0, "Long reset list can scroll");
                popup.UpdateSnapshot(manyResets.Clone());
                Pump();
                Check(Math.Abs(scroll.VerticalOffset - offset) < 1, "Refreshing preserves popup scroll position");
                var lightSettings = settings.Clone();
                lightSettings.Theme = AppSettings.ThemeLight;
                popup.ApplySettings(lightSettings);
                popup.UpdateSnapshot(snapshot.Clone());
                Pump();
                CaptureWpf(host, output, "popup-light-wpf");
                var partial = snapshot.Clone();
                partial.Weekly = null;
                partial.IsStale = true;
                partial.ErrorMessage = "Synthetic refresh failure";
                popup.UpdateSnapshot(partial);
                Pump();
                Check(!HasText(view, "Weekly") && HasText(view, "5-hour") &&
                    popup.AccessibleDescription.Contains("Synthetic refresh failure"),
                    "Partial stale snapshot hides the unavailable limit and retains its error");
            }
            });

            RunPart("Settings", failures, delegate
            {
            bool saved = false, applied = false;
            AppSettings draft = settings.Clone(), savedCandidate = null;
            using (var settingsForm = new SettingsForm(draft, delegate(AppSettings candidate) { saved = true; savedCandidate = candidate.Clone(); }))
            {
                settingsForm.SettingsApplied += delegate { applied = true; };
                settingsForm.Show();
                Pump();
                CaptureForm(settingsForm, output, "settings");
                var settingsHost = RequireHost(settingsForm, "settings");
                var view = (WpfSettingsView)settingsHost.Child;
                CaptureWpf(settingsHost, output, "settings-wpf");
                Check(!saved, "Constructing settings does not save");
                SelectSettingsPage(view, "Forecast");
                Check(view.ForecastPanel.Visibility == Visibility.Visible && view.PageTitle.Text == "Forecast & graphs", "Forecast navigation");
                CaptureWpf(settingsHost, output, "settings-forecast-wpf");
                view.QuietHours.IsDropDownOpen = true;
                Pump();
                Check(view.QuietHours.IsDropDownOpen, "Quiet-hours dropdown opens");
                bool focused = view.QuietHours.Focus();
                SendKey(view.QuietHours, Key.Down);
                Pump();
                SendKey(view.QuietHours, Key.Enter);
                Pump();
                bool routedKeyChangedSelection = view.QuietHours.SelectedIndex == 1;
                File.WriteAllText(Path.Combine(output, "keyboard-diagnostic.txt"),
                    "Quiet-hours dropdown opened; control focus=" + focused + "; routed Down then Enter changed selection=" + routedKeyChangedSelection + ".\n");
                // A non-input desktop may decline keyboard focus. Still verify the
                // selected value and dependency through the normal change event.
                if (!routedKeyChangedSelection) view.QuietHours.SelectedIndex = 1;
                Check(view.QuietHours.SelectedIndex == 1 && !view.HighlightQuiet.IsEnabled, "Quiet-hours Off updates dependent setting");
                view.QuietHours.IsDropDownOpen = false;
                view.QuietHours.SelectedIndex = 0;
                Check(view.QuietHours.SelectedIndex == 0, "Quiet-hours automatic can be restored");
                SelectSettingsPage(view, "Appearance");
                view.ThemeChoice.SelectedIndex = 2;
                Pump();
                CaptureWpf(settingsHost, output, "settings-appearance-light-wpf");
                SelectSettingsPage(view, "Usage");
                CaptureWpf(settingsHost, output, "settings-usage-light-wpf");
                SelectSettingsPage(view, "Appearance");
                view.ThemeChoice.SelectedIndex = 1;
                Pump();
                CaptureWpf(settingsHost, output, "settings-appearance-dark-wpf");
                CheckColorDialogCancel(view);
                SelectSettingsPage(view, "Usage");
                settingsForm.Size = settingsForm.MinimumSize;
                Pump();
                CaptureWpf(settingsHost, output, "settings-minimum-wpf");
                settingsForm.WpfRestoreDefaults();
                Check(view.MetricWeekly.IsChecked == true && view.HighlightQuiet.IsChecked == false, "Restore uses application defaults");
                view.MetricAuto.IsChecked = true;
                view.ThresholdAlerts.IsChecked = true;
                Pump();
                var numericEditor = FindVisual<TextBox>(view.CriticalValue);
                Check(numericEditor != null, "Threshold number box exposes its text editor");
                numericEditor.Text = "18";
                var inputSource = PresentationSource.FromVisual(numericEditor);
                Check(inputSource != null, "Threshold editor is connected to a presentation source");
                numericEditor.RaiseEvent(new KeyEventArgs(Keyboard.PrimaryDevice, inputSource, 0, Key.Enter)
                    { RoutedEvent = Keyboard.PreviewKeyUpEvent });
                Pump();
                Check(view.CriticalValue.Value == 18, "Threshold text is committed by Enter");
                view.CriticalValue.Value = 80;
                Check(view.LowValue.Value >= view.CriticalValue.Value, "Threshold values stay in valid order");
                view.ReminderEnabled.IsChecked = false;
                view.ShowProjection.IsChecked = false;
                Check(!view.HighlightQuiet.IsEnabled, "Hidden projection disables quiet highlighting");
                Check(draft.IconMetric == AppSettings.IconMetricWeekly && !saved, "Draft edits do not mutate live settings");
                var saveAction = FindAction(view, "Save settings");
                Check(saveAction != null, "Settings Save action is accessible");
                saveAction.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                Check(saved && applied && savedCandidate != null && savedCandidate.IconMetric == AppSettings.IconMetricAuto &&
                    savedCandidate.CriticalThreshold == 80 && savedCandidate.LowThreshold >= 80 &&
                    !savedCandidate.ResetExpiryReminders && !savedCandidate.ShowProjectedUsage &&
                    draft.IconMetric == AppSettings.IconMetricAuto, "Saving applies the edited candidate exactly once");
            }
            bool cancelledSaved = false;
            var cancelledDraft = settings.Clone();
            using (var cancelForm = new SettingsForm(cancelledDraft, delegate(AppSettings ignored) { cancelledSaved = true; }))
            {
                cancelForm.Show();
                Pump();
                var view = (WpfSettingsView)RequireHost(cancelForm, "settings cancel").Child;
                view.MetricAuto.IsChecked = true;
                cancelForm.Close();
                Check(!cancelledSaved && cancelledDraft.IconMetric == AppSettings.IconMetricWeekly, "Cancel discards draft edits");
            }
            });

            RunPart("History", failures, delegate
            {
            using (var form = new UsageHistoryForm(history, settings))
            {
                form.UpdateData(snapshot);
                form.Show();
                Pump();
                var headerHost = RequireHost(form, "history");
                var header = (FrameworkElement)headerHost.Child;
                var remaining = (TextBlock)header.FindName("Remaining");
                var fiveHour = (RadioButton)header.FindName("FiveHour");
                var weekly = (RadioButton)header.FindName("Weekly");
                Check(remaining != null && remaining.Text.Contains("72"), "Weekly header uses the live sample");
                CaptureForm(form, output, "history-weekly");
                CaptureWpf(headerHost, output, "history-weekly-header");
                Check(fiveHour != null && weekly != null, "History period controls are available");
                fiveHour.IsChecked = true;
                Pump();
                Check(remaining.Text.Contains("65"), "Five-hour selection updates the live remaining value");
                CaptureForm(form, output, "history-five-hour");
                CaptureWpf(headerHost, output, "history-five-hour-header");
                weekly.IsChecked = true;
                Pump();
                Check(remaining.Text.Contains("72"), "Weekly selection restores the live value");
                form.Size = form.MinimumSize;
                Pump();
                CaptureForm(form, output, "history-minimum");
                CaptureWpf(headerHost, output, "history-minimum-header");
            }
            });
            if (failures.Count != 0) throw new InvalidOperationException(string.Join(Environment.NewLine, failures));
            File.WriteAllText(Path.Combine(output, "result.txt"),
                "Private-desktop WPF construction, sample snapshot presentation, history period switching and minimum-size capture passed.\n");
        }

        private static void RunPart(string name, List<string> failures, Action action)
        {
            try { action(); }
            catch (Exception error) { failures.Add(name + ": " + error); }
        }

        private static void SelectSettingsPage(WpfSettingsView view, string tag)
        {
            foreach (object item in view.Navigation.Items)
            {
                var navigationItem = item as ListBoxItem;
                if (navigationItem != null && (string)navigationItem.Tag == tag)
                {
                    view.Navigation.SelectedItem = navigationItem;
                    Pump();
                    return;
                }
            }
            throw new InvalidOperationException("Missing settings page: " + tag);
        }

        private static void SendKey(System.Windows.UIElement control, Key key)
        {
            var source = PresentationSource.FromVisual(control);
            Check(source != null, "Keyboard target is connected to a presentation source");
            var keyDown = new KeyEventArgs(Keyboard.PrimaryDevice, source, 0, key) { RoutedEvent = Keyboard.KeyDownEvent };
            control.RaiseEvent(keyDown);
        }

        private static void CheckColorDialogCancel(WpfSettingsView view)
        {
            string before = view.TrayBoxHex.Text;
            bool cancelled = false;
            using (var timer = new Forms.Timer { Interval = 100 })
            {
                timer.Tick += delegate
                {
                    EnumThreadWindows(GetCurrentThreadId(), delegate(IntPtr window, IntPtr parameter)
                    {
                        var className = new StringBuilder(64);
                        GetClassName(window, className, className.Capacity);
                        if (className.ToString() != "#32770") return true;
                        IntPtr cancelButton = GetDlgItem(window, 2);
                        if (cancelButton == IntPtr.Zero) return true;
                        cancelled = PostMessage(cancelButton, 0x00F5, IntPtr.Zero, IntPtr.Zero); // BM_CLICK
                        if (cancelled) timer.Stop();
                        return !cancelled;
                    }, IntPtr.Zero);
                };
                timer.Start();
                view.TrayBoxColor.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                timer.Stop();
            }
            Check(cancelled && view.TrayBoxHex.Text == before, "Cancelling the real color dialog leaves the draft color unchanged");
        }

        private static void Pump()
        {
            Forms.Application.DoEvents();
            Dispatcher.CurrentDispatcher.Invoke(DispatcherPriority.ApplicationIdle, new Action(delegate { }));
            Forms.Application.DoEvents();
        }

        private static ElementHost RequireHost(Forms.Control parent, string description)
        {
            foreach (Forms.Control control in parent.Controls)
            {
                var host = control as ElementHost;
                if (host != null && host.Child != null && host.Visible) return host;
                var nested = FindHost(control);
                if (nested != null) return nested;
            }
            throw new InvalidOperationException("No visible WPF host for " + description + ".");
        }

        private static ElementHost FindHost(Forms.Control parent)
        {
            foreach (Forms.Control control in parent.Controls)
            {
                var host = control as ElementHost;
                if (host != null && host.Child != null && host.Visible) return host;
                var nested = FindHost(control);
                if (nested != null) return nested;
            }
            return null;
        }

        private static ButtonBase FindAction(DependencyObject parent, string name)
        {
            var button = parent as ButtonBase;
            if (button != null && AutomationProperties.GetName(button) == name) return button;
            for (int index = 0; index < VisualTreeHelper.GetChildrenCount(parent); index++)
            {
                var found = FindAction(VisualTreeHelper.GetChild(parent, index), name);
                if (found != null) return found;
            }
            return null;
        }

        private static bool HasText(DependencyObject parent, string value)
        {
            var text = parent as TextBlock;
            if (text != null && text.Text == value) return true;
            for (int index = 0; index < VisualTreeHelper.GetChildrenCount(parent); index++)
                if (HasText(VisualTreeHelper.GetChild(parent, index), value)) return true;
            return false;
        }

        private static bool HasAutomationName(DependencyObject parent, string value)
        {
            if (AutomationProperties.GetName(parent) == value) return true;
            for (int index = 0; index < VisualTreeHelper.GetChildrenCount(parent); index++)
                if (HasAutomationName(VisualTreeHelper.GetChild(parent, index), value)) return true;
            return false;
        }

        private static void CheckDailyTooltipCases()
        {
            var fixture = new UsagePopupView();
            var state = new UsagePopupView.State
            {
                Provider = "Codex",
                Dark = true,
                Daily = new double?[] { 0, 0.4, null, 130, 0.0001, 2, 3 },
                DailyDates = new DateTime[7],
                DailyPartial = new bool[] { true, true, true, false, true, false, false }
            };
            for (int day = 0; day < 7; day++) state.DailyDates[day] = DateTime.Today.AddDays(day - 6);
            fixture.Update(state);
            fixture.Measure(new System.Windows.Size(520, 300));
            fixture.Arrange(new Rect(0, 0, 520, 300));
            fixture.UpdateLayout();
            var bars = FindVisual<UniformGrid>(fixture);
            Check(bars != null && bars.Children.Count == 7, "Fixture exposes all seven daily bars");
            Check(DailyTip(bars, 0).Contains("Observed: 0% used") && DailyTip(bars, 0).Contains("Partial coverage"),
                "Observed zero is explicit and reports partial coverage");
            string fraction = 0.4.ToString("0.###", CultureInfo.CurrentCulture);
            Check(DailyTip(bars, 1).Contains("Observed: " + fraction + "% used"), "Small nonzero usage is not rounded to zero");
            Check(DailyTip(bars, 2).Contains("Usage unavailable"), "Missing observations are not reported as zero");
            Check(DailyTip(bars, 3).Contains("Observed: 130% used") && !DailyTip(bars, 3).Contains("Partial coverage"),
                "Usage over 100 percent and full coverage remain distinct");
            Check(DailyTip(bars, 4).Contains("<0.001% used"), "Very small positive usage is distinguishable from zero");
        }

        private static string DailyTip(UniformGrid grid, int index)
        {
            var button = (System.Windows.Controls.Button)grid.Children[index];
            var tooltip = (ToolTip)button.ToolTip;
            return ((TextBlock)tooltip.Content).Text;
        }

        private static T FindVisual<T>(DependencyObject parent) where T : DependencyObject
        {
            if (parent is T) return (T)parent;
            for (int index = 0; index < VisualTreeHelper.GetChildrenCount(parent); index++)
            {
                var found = FindVisual<T>(VisualTreeHelper.GetChild(parent, index));
                if (found != null) return found;
            }
            return null;
        }

        private static void CaptureWpf(ElementHost host, string directory, string name)
        {
            var child = host.Child;
            child.UpdateLayout();
            int width = (int)Math.Ceiling(child.RenderSize.Width);
            int height = (int)Math.Ceiling(child.RenderSize.Height);
            Check(width > 0 && height > 0, name + " has drawable dimensions");
            var bitmap = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
            bitmap.Render(child);
            var encoder = new PngBitmapEncoder();
            encoder.Frames.Add(BitmapFrame.Create(bitmap));
            using (var stream = new FileStream(Path.Combine(directory, name + ".png"), FileMode.CreateNew)) encoder.Save(stream);
        }

        private static void CaptureVisual(FrameworkElement element, string directory, string name)
        {
            element.UpdateLayout();
            int width = (int)Math.Ceiling(element.ActualWidth), height = (int)Math.Ceiling(element.ActualHeight);
            Check(width > 0 && height > 0, name + " has drawable dimensions");
            // Tooltip templates can render a drop-shadow outside their layout
            // bounds; leave room for that effect in the exported visual.
            var bitmap = new RenderTargetBitmap(width + 32, height + 32, 96, 96, PixelFormats.Pbgra32);
            bitmap.Render(element);
            var encoder = new PngBitmapEncoder();
            encoder.Frames.Add(BitmapFrame.Create(bitmap));
            using (var stream = new FileStream(Path.Combine(directory, name + ".png"), FileMode.CreateNew)) encoder.Save(stream);
        }

        private static void CaptureForm(Forms.Form form, string directory, string name)
        {
            Check(form.Width > 0 && form.Height > 0, name + " has drawable dimensions");
            using (var bitmap = new Bitmap(form.Width, form.Height, System.Drawing.Imaging.PixelFormat.Format32bppArgb))
            using (var graphics = Graphics.FromImage(bitmap))
            {
                IntPtr dc = graphics.GetHdc();
                try
                {
                    if (!PrintWindow(form.Handle, dc, 2)) throw new Win32Exception(Marshal.GetLastWin32Error());
                }
                finally { graphics.ReleaseHdc(dc); }
                bitmap.Save(Path.Combine(directory, name + ".png"), ImageFormat.Png);
                // PrintWindow may omit an ElementHost surface on an inactive desktop.
                // Preserve the raw capture above and additionally overlay the separately
                // rendered WPF visual at the host's actual client bounds for comparison.
                ElementHost host = FindHost(form);
                if (host != null && host.Child != null && host.Width > 0 && host.Height > 0)
                {
                    var child = host.Child;
                    child.UpdateLayout();
                    int width = (int)Math.Ceiling(child.RenderSize.Width);
                    int height = (int)Math.Ceiling(child.RenderSize.Height);
                    if (width > 0 && height > 0)
                    {
                        var wpfImage = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
                        wpfImage.Render(child);
                        var encoder = new PngBitmapEncoder();
                        encoder.Frames.Add(BitmapFrame.Create(wpfImage));
                        using (var buffer = new MemoryStream())
                        {
                            encoder.Save(buffer);
                            buffer.Position = 0;
                            using (var overlay = System.Drawing.Image.FromStream(buffer))
                            {
                                System.Drawing.Point origin = host.PointToScreen(System.Drawing.Point.Empty);
                                graphics.DrawImage(overlay, origin.X - form.Left, origin.Y - form.Top, host.Width, host.Height);
                            }
                        }
                        bitmap.Save(Path.Combine(directory, name + "-composite.png"), ImageFormat.Png);
                    }
                }
            }
        }

        private static void Check(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("Integration check failed: " + message);
        }

        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool PrintWindow(IntPtr window, IntPtr deviceContext, uint flags);
        [StructLayout(LayoutKind.Sequential)] private struct NativeRect { public int Left, Top, Right, Bottom; }
        [DllImport("user32.dll", SetLastError = true)] private static extern bool GetWindowRect(IntPtr window, out NativeRect bounds);
        private delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);
        [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
        [DllImport("user32.dll")] private static extern bool EnumThreadWindows(uint threadId, EnumWindowsCallback callback, IntPtr parameter);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder name, int maxCount);
        [DllImport("user32.dll")] private static extern IntPtr GetDlgItem(IntPtr dialog, int id);
        [DllImport("user32.dll", SetLastError = true)] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    }
}
