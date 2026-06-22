using System;
using System.Collections.Generic;
using System.Collections.Specialized;
using System.Diagnostics;
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
    internal sealed class TrayAppContext : ApplicationContext
    {
        private const int DefaultRefreshSeconds = 300;

        private readonly NotifyIcon notifyIcon;
        private readonly System.Windows.Forms.Timer refreshTimer;
        private readonly Control dispatcher;
        private readonly AppSettings settings;
        private readonly UsagePopup usagePopup;
        private readonly bool showUsageOnStart;
        private readonly bool showSettingsOnStart;

        private Icon currentIcon;
        private UsageSnapshot lastSnapshot;
        private bool refreshInProgress;
        private bool updateCheckInProgress;
        private bool updateInstallInProgress;
        private SettingsForm settingsForm;
        private System.Windows.Forms.Timer startupUiTimer;

        public TrayAppContext(string[] args)
        {
            showUsageOnStart = HasArg(args, "--show-usage");
            showSettingsOnStart = HasArg(args, "--show-settings");
            dispatcher = new Control();
            dispatcher.CreateControl();
            settings = AppSettings.Load();
            usagePopup = new UsagePopup(settings);
            usagePopup.SettingsRequested += delegate
            {
                usagePopup.Hide();
                ShowSettings();
            };

            notifyIcon = new NotifyIcon();
            SetNotifyTooltip("Codex Usage: starting");
            notifyIcon.ContextMenuStrip = BuildContextMenu();
            notifyIcon.MouseClick += NotifyIcon_MouseClick;
            SetTrayIcon(IconRenderer.CreateUnknownIcon("..."));
            notifyIcon.Visible = true;

            refreshTimer = new System.Windows.Forms.Timer();
            refreshTimer.Interval = Math.Max(30, settings.RefreshSeconds) * 1000;
            refreshTimer.Tick += delegate { RefreshUsage(false); };
            refreshTimer.Start();

            RefreshUsage(false);
            CheckForUpdates(false);
            if (showUsageOnStart || showSettingsOnStart)
            {
                startupUiTimer = new System.Windows.Forms.Timer();
                startupUiTimer.Interval = 2500;
                startupUiTimer.Tick += delegate
                {
                    startupUiTimer.Stop();
                    startupUiTimer.Dispose();
                    startupUiTimer = null;
                    if (showUsageOnStart)
                    {
                        ShowUsagePopup();
                    }
                    if (showSettingsOnStart)
                    {
                        ShowSettings();
                    }
                };
                startupUiTimer.Start();
            }
        }

        private static bool HasArg(string[] args, string value)
        {
            if (args == null)
            {
                return false;
            }

            for (int i = 0; i < args.Length; i++)
            {
                if (string.Equals(args[i], value, StringComparison.OrdinalIgnoreCase))
                {
                    return true;
                }
            }

            return false;
        }

        private ContextMenuStrip BuildContextMenu()
        {
            ContextMenuStrip menu = new ContextMenuStrip();

            ToolStripMenuItem refresh = new ToolStripMenuItem("Refresh now");
            refresh.Click += delegate { RefreshUsage(true); };
            menu.Items.Add(refresh);

            ToolStripMenuItem show = new ToolStripMenuItem("Show usage");
            show.Click += delegate { ShowUsagePopup(); };
            menu.Items.Add(show);

            ToolStripMenuItem settingsItem = new ToolStripMenuItem("Settings");
            settingsItem.Click += delegate { ShowSettings(); };
            menu.Items.Add(settingsItem);

            menu.Items.Add(new ToolStripSeparator());

            ToolStripMenuItem exit = new ToolStripMenuItem("Exit");
            exit.Click += delegate { ExitThread(); };
            menu.Items.Add(exit);

            return menu;
        }

        private void NotifyIcon_MouseClick(object sender, MouseEventArgs e)
        {
            if (e.Button == MouseButtons.Left)
            {
                ToggleUsagePopup();
            }
        }

        private void RefreshUsage(bool showBalloon)
        {
            if (refreshInProgress)
            {
                return;
            }

            refreshInProgress = true;
            SetNotifyTooltip("Codex Usage: refreshing");

            Task.Factory.StartNew(
                delegate { return CodexRateLimitClient.FetchUsage(); },
                CancellationToken.None,
                TaskCreationOptions.None,
                TaskScheduler.Default).ContinueWith(delegate(Task<UsageSnapshot> task)
            {
                if (!dispatcher.IsDisposed && dispatcher.IsHandleCreated)
                {
                    dispatcher.BeginInvoke(new Action(delegate
                    {
                        refreshInProgress = false;
                        if (task.IsFaulted)
                        {
                            string message = task.Exception != null && task.Exception.GetBaseException() != null
                                ? task.Exception.GetBaseException().Message
                                : "Unknown error";
                            ApplySnapshot(UsageSnapshot.FromError(message), showBalloon);
                        }
                        else
                        {
                            ApplySnapshot(task.Result, showBalloon);
                        }
                    }));
                }
            });
        }

        private void ApplySnapshot(UsageSnapshot snapshot, bool showBalloon)
        {
            lastSnapshot = snapshot;

            if (!string.IsNullOrEmpty(snapshot.ErrorMessage))
            {
                SetTrayIcon(IconRenderer.CreateErrorIcon());
                SetNotifyTooltip("Codex Usage: " + snapshot.ErrorMessage);
                if (showBalloon)
                {
                    notifyIcon.ShowBalloonTip(3000, "Codex Usage", snapshot.ErrorMessage, ToolTipIcon.Warning);
                }
                return;
            }

            LimitWindow weekly = snapshot.Weekly;
            LimitWindow iconWindow = GetIconWindow(snapshot);
            int remaining = iconWindow != null ? ClampPercent(100.0 - iconWindow.UsedPercent) : 0;
            SetTrayIcon(IconRenderer.CreatePercentIcon(remaining, settings));
            SetNotifyTooltip(BuildNativeTooltip(snapshot));
            usagePopup.UpdateSnapshot(snapshot);

            if (showBalloon)
            {
                notifyIcon.ShowBalloonTip(2500, "Codex Usage", BuildTooltip(snapshot), ToolTipIcon.Info);
            }
        }

        private static int ClampPercent(double value)
        {
            if (value < 0)
            {
                return 0;
            }
            if (value > 100)
            {
                return 100;
            }
            return (int)Math.Round(value);
        }

        private static string BuildTooltip(UsageSnapshot snapshot)
        {
            return "Weekly " + FormatWindow(snapshot.Weekly) + " | 5h " + FormatWindow(snapshot.FiveHour);
        }

        private static string BuildNativeTooltip(UsageSnapshot snapshot)
        {
            return "Codex Usage Remaining" + Environment.NewLine +
                "Weekly: " + FormatNativeWindow(snapshot.Weekly) + Environment.NewLine +
                "5h: " + FormatNativeWindow(snapshot.FiveHour);
        }

        private static string FormatWindow(LimitWindow window)
        {
            if (window == null)
            {
                return "unknown";
            }

            int remaining = ClampPercent(100.0 - window.UsedPercent);
            string reset = window.ResetAfterSeconds.HasValue
                ? ", resets " + TimeFormatter.FormatDuration(window.ResetAfterSeconds.Value)
                : "";
            return remaining + "% left" + reset;
        }

        private static string FormatNativeWindow(LimitWindow window)
        {
            if (window == null)
            {
                return "unknown";
            }

            string reset = window.ResetAfterSeconds.HasValue
                ? " (Resets in " + TimeFormatter.FormatDuration(window.ResetAfterSeconds.Value) + ")"
                : "";
            return window.RemainingPercent + "%" + reset;
        }

        private static string TrimTooltip(string value)
        {
            if (string.IsNullOrEmpty(value))
            {
                return "Codex Usage";
            }

            return value.Length > 63 ? value.Substring(0, 63) : value;
        }

        private void SetNotifyTooltip(string value)
        {
            notifyIcon.Text = TrimTooltip(FirstTooltipLine(value));
            NativeTrayTooltip.TrySetText(notifyIcon, value);
        }

        private static string FirstTooltipLine(string value)
        {
            if (string.IsNullOrEmpty(value))
            {
                return "Codex Usage";
            }

            int lineBreak = value.IndexOfAny(new char[] { '\r', '\n' });
            return lineBreak >= 0 ? value.Substring(0, lineBreak) : value;
        }

        private void SetTrayIcon(Icon icon)
        {
            Icon oldIcon = currentIcon;
            currentIcon = icon;
            notifyIcon.Icon = currentIcon;
            if (oldIcon != null)
            {
                oldIcon.Dispose();
            }
        }

        private LimitWindow GetIconWindow(UsageSnapshot snapshot)
        {
            if (snapshot == null)
            {
                return null;
            }

            return string.Equals(settings.IconMetric, AppSettings.IconMetricFiveHour, StringComparison.OrdinalIgnoreCase)
                ? snapshot.FiveHour
                : snapshot.Weekly;
        }

        private void ToggleUsagePopup()
        {
            if (usagePopup.Visible)
            {
                usagePopup.Hide();
            }
            else
            {
                ShowUsagePopup();
            }
        }

        private void ShowUsagePopup()
        {
            if (lastSnapshot == null)
            {
                usagePopup.UpdateSnapshot(UsageSnapshot.FromError("Checking limits..."));
            }
            else
            {
                usagePopup.UpdateSnapshot(lastSnapshot);
            }
            usagePopup.ShowNear(Cursor.Position);
        }

        private void ShowSettings()
        {
            if (settingsForm != null && !settingsForm.IsDisposed)
            {
                settingsForm.Activate();
                return;
            }

            settingsForm = new SettingsForm(settings);
            settingsForm.SettingsApplied += delegate
            {
                settings.Save();
                refreshTimer.Interval = Math.Max(30, settings.RefreshSeconds) * 1000;
                usagePopup.ApplySettings(settings);
                if (lastSnapshot != null)
                {
                    ApplySnapshot(lastSnapshot, false);
                }
            };
            settingsForm.CheckUpdatesRequested += delegate { CheckForUpdates(true); };
            settingsForm.Show();
        }

        private void CheckForUpdates(bool interactive)
        {
            if (updateCheckInProgress || updateInstallInProgress)
            {
                if (interactive)
                {
                    MessageBox.Show(GetSettingsOwner(), "An update check is already running.", "Codex Usage Updates", MessageBoxButtons.OK, MessageBoxIcon.Information);
                }
                return;
            }

            updateCheckInProgress = true;
            SetSettingsUpdateButtonState(true, "Checking...");

            Task.Factory.StartNew(
                delegate { return UpdateService.CheckForUpdate(); },
                CancellationToken.None,
                TaskCreationOptions.None,
                TaskScheduler.Default).ContinueWith(delegate(Task<UpdateInfo> task)
            {
                if (!dispatcher.IsDisposed && dispatcher.IsHandleCreated)
                {
                    dispatcher.BeginInvoke(new Action(delegate
                    {
                        updateCheckInProgress = false;
                        SetSettingsUpdateButtonState(false, null);
                        if (task.IsFaulted)
                        {
                            HandleUpdateError(task.Exception, interactive);
                        }
                        else
                        {
                            HandleUpdateInfo(task.Result, interactive);
                        }
                    }));
                }
            });
        }

        private void HandleUpdateInfo(UpdateInfo info, bool interactive)
        {
            if (info == null)
            {
                if (interactive)
                {
                    MessageBox.Show(GetSettingsOwner(), "Could not read update information.", "Codex Usage Updates", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                }
                return;
            }

            if (!info.UpdateAvailable)
            {
                if (interactive)
                {
                    MessageBox.Show(GetSettingsOwner(), info.Message, "Codex Usage Updates", MessageBoxButtons.OK, MessageBoxIcon.Information);
                }
                return;
            }

            if (!interactive)
            {
                notifyIcon.ShowBalloonTip(
                    5000,
                    "Codex Usage Update",
                    "Version " + info.LatestVersion + " is available. Open Settings and choose Check Updates to install it.",
                    ToolTipIcon.Info);
                return;
            }

            if (info.CanInstall)
            {
                DialogResult result = MessageBox.Show(
                    GetSettingsOwner(),
                    info.Message + Environment.NewLine + Environment.NewLine + "Install now? Codex Usage Tray will restart automatically.",
                    "Codex Usage Updates",
                    MessageBoxButtons.YesNo,
                    MessageBoxIcon.Information);
                if (result == DialogResult.Yes)
                {
                    InstallUpdate(info);
                }
                return;
            }

            DialogResult openRelease = MessageBox.Show(
                GetSettingsOwner(),
                info.Message + Environment.NewLine + Environment.NewLine + "Open the release page?",
                "Codex Usage Updates",
                MessageBoxButtons.YesNo,
                MessageBoxIcon.Information);
            if (openRelease == DialogResult.Yes)
            {
                OpenUrl(info.ReleaseUrl);
            }
        }

        private void InstallUpdate(UpdateInfo info)
        {
            if (updateInstallInProgress)
            {
                return;
            }

            updateInstallInProgress = true;
            SetSettingsUpdateButtonState(true, "Installing...");

            Task.Factory.StartNew(
                delegate { return UpdateService.InstallUpdate(info); },
                CancellationToken.None,
                TaskCreationOptions.None,
                TaskScheduler.Default).ContinueWith(delegate(Task<string> task)
            {
                if (!dispatcher.IsDisposed && dispatcher.IsHandleCreated)
                {
                    dispatcher.BeginInvoke(new Action(delegate
                    {
                        if (task.IsFaulted)
                        {
                            updateInstallInProgress = false;
                            SetSettingsUpdateButtonState(false, null);
                            HandleUpdateError(task.Exception, true);
                            return;
                        }

                        notifyIcon.ShowBalloonTip(2000, "Codex Usage Updates", task.Result, ToolTipIcon.Info);
                        ExitThread();
                    }));
                }
            });
        }

        private void HandleUpdateError(AggregateException exception, bool interactive)
        {
            if (!interactive)
            {
                return;
            }

            Exception baseException = exception != null ? exception.GetBaseException() : null;
            string message = baseException != null ? baseException.Message : "Unknown update error.";
            MessageBox.Show(GetSettingsOwner(), "Could not check for updates:" + Environment.NewLine + Environment.NewLine + message, "Codex Usage Updates", MessageBoxButtons.OK, MessageBoxIcon.Warning);
        }

        private IWin32Window GetSettingsOwner()
        {
            return settingsForm != null && !settingsForm.IsDisposed ? settingsForm : null;
        }

        private void SetSettingsUpdateButtonState(bool busy, string busyText)
        {
            if (settingsForm != null && !settingsForm.IsDisposed)
            {
                settingsForm.SetUpdateButtonState(busy, busyText);
            }
        }

        private void OpenUrl(string url)
        {
            if (string.IsNullOrWhiteSpace(url))
            {
                return;
            }

            try
            {
                ProcessStartInfo startInfo = new ProcessStartInfo(url);
                startInfo.UseShellExecute = true;
                Process.Start(startInfo);
            }
            catch (Exception ex)
            {
                MessageBox.Show(GetSettingsOwner(), "Could not open release page:" + Environment.NewLine + Environment.NewLine + ex.Message, "Codex Usage Updates", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            }
        }

        protected override void ExitThreadCore()
        {
            refreshTimer.Stop();
            refreshTimer.Dispose();
            if (startupUiTimer != null)
            {
                startupUiTimer.Stop();
                startupUiTimer.Dispose();
            }
            usagePopup.Dispose();
            if (settingsForm != null)
            {
                settingsForm.Dispose();
            }
            notifyIcon.Visible = false;
            notifyIcon.Dispose();
            if (currentIcon != null)
            {
                currentIcon.Dispose();
            }
            dispatcher.Dispose();
            base.ExitThreadCore();
        }
    }
}
