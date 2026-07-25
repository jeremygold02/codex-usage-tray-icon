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
        private const int ActivityPollSeconds = 30;
        private const int ShowWindowRestore = 9;
        private const string RefreshingStatus = "Refreshing usage...";
        private const string RefreshFailedStatus = "Refresh failed; showing last known usage.";
        private const string RefreshFailedWithoutDataStatus = "Unable to refresh usage. Try again.";
        private const string RefreshCanceledStatus = "Usage refresh was canceled. Try again.";
        private const string RefreshReturnedNoDataStatus = "Codex usage check returned no result.";
        private const string UnexpectedRefreshFailureStatus = "Unexpected error while refreshing usage.";
        private const string PausedStatus = "Automatic refresh paused while Codex is not running.";
        private const string PausedAfterFailureStatus = "Automatic refresh paused; last refresh failed.";
        private const string WaitingStatus = "Waiting for Codex to start...";

        private readonly NotifyIcon notifyIcon;
        private readonly System.Windows.Forms.Timer refreshTimer;
        private readonly System.Windows.Forms.Timer activityTimer;
        private readonly Control dispatcher;
        private readonly AppSettings settings;
        private readonly Stopwatch refreshClock;
        private readonly CancellationTokenSource shutdownCancellation;
        private readonly UsageResetDetector usageResetDetector;
        private readonly bool showUsageOnStart;
        private readonly bool showSettingsOnStart;

        private UsagePopup usagePopup;
        private Icon currentIcon;
        private UsageSnapshot lastSuccessfulSnapshot;
        private UsageSnapshot currentSnapshot;
        private int? lastWeeklyThresholdLevel;
        private int? lastFiveHourThresholdLevel;
        private long lastRefreshStartedMilliseconds = -1;
        private DateTime lastAttemptedAt = DateTime.MinValue;
        private bool codexRunning;
        private bool refreshInProgress;
        private bool refreshFeedbackRequested;
        private bool updateCheckInProgress;
        private bool updateInstallInProgress;
        private volatile bool shuttingDown;
        private string updateStatusText = "";
        private SettingsForm settingsForm;
        private System.Windows.Forms.Timer startupUiTimer;

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool ShowWindow(IntPtr windowHandle, int command);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetForegroundWindow(IntPtr windowHandle);

        public TrayAppContext(string[] args)
        {
            showUsageOnStart = HasArg(args, "--show-usage");
            showSettingsOnStart = HasArg(args, "--show-settings");
            refreshClock = Stopwatch.StartNew();
            shutdownCancellation = new CancellationTokenSource();
            usageResetDetector = new UsageResetDetector();
            dispatcher = new Control();
            dispatcher.CreateControl();
            settings = AppSettings.Load();
            usagePopup = CreateUsagePopup();

            notifyIcon = new NotifyIcon();
            SetNotifyTooltip("Codex Usage: starting");
            notifyIcon.ContextMenuStrip = BuildContextMenu();
            notifyIcon.MouseClick += NotifyIcon_MouseClick;
            SetTrayIcon(IconRenderer.CreateUnknownIcon("..."));
            notifyIcon.Visible = true;

            refreshTimer = new System.Windows.Forms.Timer();
            refreshTimer.Tick += delegate
            {
                refreshTimer.Stop();
                RefreshUsageIfDue(false);
            };

            codexRunning = CodexActivityMonitor.IsCodexRunning();
            activityTimer = new System.Windows.Forms.Timer();
            activityTimer.Interval = ActivityPollSeconds * 1000;
            activityTimer.Tick += delegate { PollCodexActivity(); };
            activityTimer.Start();

            RefreshUsageIfDue(false);
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

        public void RequestExit()
        {
            if (shuttingDown)
            {
                return;
            }

            TryPostToUi(delegate
            {
                if (!shuttingDown)
                {
                    ExitThread();
                }
            });
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
            if (shuttingDown)
            {
                return;
            }

            refreshFeedbackRequested = refreshFeedbackRequested || showBalloon;
            if (refreshInProgress)
            {
                SetUsagePopupRefreshing(true);
                return;
            }

            refreshInProgress = true;
            lastAttemptedAt = DateTime.Now;
            lastRefreshStartedMilliseconds = refreshClock.ElapsedMilliseconds;
            refreshTimer.Stop();
            ApplyRefreshingState();

            CancellationToken cancellationToken = shutdownCancellation.Token;
            Task.Factory.StartNew(
                delegate
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    return CodexRateLimitClient.FetchUsage();
                },
                cancellationToken,
                TaskCreationOptions.None,
                TaskScheduler.Default).ContinueWith(delegate(Task<UsageSnapshot> task)
            {
                if (task.IsFaulted)
                {
                    AggregateException ignored = task.Exception;
                }

                TryPostToUi(delegate
                {
                    if (shuttingDown)
                    {
                        return;
                    }
                    CompleteRefresh(task);
                });
            }, CancellationToken.None, TaskContinuationOptions.None, TaskScheduler.Default);
        }

        private void RefreshUsageIfDue(bool showBalloon)
        {
            if (shuttingDown)
            {
                return;
            }

            codexRunning = CodexActivityMonitor.IsCodexRunning();
            int refreshSeconds = GetCurrentRefreshSeconds();
            if (refreshSeconds <= 0)
            {
                ApplyPausedState();
                return;
            }

            RestoreActiveState();
            if (refreshInProgress)
            {
                return;
            }

            if (lastRefreshStartedMilliseconds < 0)
            {
                RefreshUsage(showBalloon);
                return;
            }

            long intervalMilliseconds = (long)refreshSeconds * 1000L;
            long elapsedMilliseconds = refreshClock.ElapsedMilliseconds - lastRefreshStartedMilliseconds;
            long remainingMilliseconds = intervalMilliseconds - elapsedMilliseconds;
            if (remainingMilliseconds <= 0)
            {
                RefreshUsage(showBalloon);
                return;
            }

            ArmRefreshTimer(remainingMilliseconds);
        }

        private int GetCurrentRefreshSeconds()
        {
            if (codexRunning)
            {
                return Math.Max(30, settings.RefreshSeconds);
            }

            return Math.Max(0, settings.IdleRefreshSeconds);
        }

        private void ArmRefreshTimer(long delayMilliseconds)
        {
            if (shuttingDown || refreshInProgress)
            {
                return;
            }

            long boundedDelay = Math.Max(1L, Math.Min((long)int.MaxValue, delayMilliseconds));
            refreshTimer.Stop();
            refreshTimer.Interval = (int)boundedDelay;
            refreshTimer.Start();
        }

        private void PollCodexActivity()
        {
            if (shuttingDown)
            {
                return;
            }

            bool isRunning = CodexActivityMonitor.IsCodexRunning();
            if (isRunning == codexRunning)
            {
                return;
            }

            codexRunning = isRunning;
            RefreshScheduleChanged();
        }

        private void RefreshScheduleChanged()
        {
            refreshTimer.Stop();
            if (GetCurrentRefreshSeconds() <= 0)
            {
                ApplyPausedState();
                return;
            }

            RestoreActiveState();
            RefreshUsageIfDue(false);
        }

        private void CompleteRefresh(Task<UsageSnapshot> task)
        {
            refreshInProgress = false;
            bool showFeedback = refreshFeedbackRequested;
            refreshFeedbackRequested = false;
            SetUsagePopupRefreshing(false);

            UsageSnapshot completedSnapshot = GetCompletedRefreshSnapshot(task);
            if (!string.IsNullOrEmpty(completedSnapshot.ErrorMessage))
            {
                ApplyRefreshFailure(completedSnapshot, showFeedback);
            }
            else
            {
                ApplySuccessfulSnapshot(completedSnapshot, showFeedback);
            }

            RefreshUsageIfDue(false);
        }

        private static UsageSnapshot GetCompletedRefreshSnapshot(Task<UsageSnapshot> task)
        {
            if (task == null)
            {
                return UsageSnapshot.FromError(UnexpectedRefreshFailureStatus);
            }
            if (task.IsCanceled)
            {
                return UsageSnapshot.FromError(RefreshCanceledStatus);
            }
            if (task.IsFaulted)
            {
                AggregateException aggregate = task.Exception;
                Exception failure = aggregate != null ? aggregate.GetBaseException() : null;
                return UsageSnapshot.FromError(GetUnexpectedRefreshMessage(failure));
            }

            UsageSnapshot snapshot = task.Result;
            if (snapshot == null)
            {
                return UsageSnapshot.FromError(RefreshReturnedNoDataStatus);
            }
            if (string.IsNullOrWhiteSpace(snapshot.ErrorMessage) && !snapshot.HasPrimaryLimit)
            {
                return UsageSnapshot.FromError(UsageSnapshot.PrimaryLimitsUnavailableMessage);
            }

            return snapshot;
        }

        private static string GetUnexpectedRefreshMessage(Exception failure)
        {
            if (failure is UnauthorizedAccessException)
            {
                return "Windows denied access while starting the Codex usage check.";
            }
            if (failure is IOException)
            {
                return "Codex usage data could not be read. Try again.";
            }

            return UnexpectedRefreshFailureStatus;
        }

        private void ApplyRefreshingState()
        {
            UsageSnapshot refreshingSnapshot;
            if (lastSuccessfulSnapshot != null)
            {
                refreshingSnapshot = lastSuccessfulSnapshot.Clone();
            }
            else
            {
                refreshingSnapshot = UsageSnapshot.FromError(RefreshingStatus);
                refreshingSnapshot.LastUpdated = DateTime.MinValue;
            }

            refreshingSnapshot.LastAttempted = lastAttemptedAt;
            refreshingSnapshot.StatusMessage = RefreshingStatus;
            refreshingSnapshot.IsStale = false;
            refreshingSnapshot.IsRefreshing = true;
            refreshingSnapshot.IsPaused = false;
            currentSnapshot = refreshingSnapshot;

            if (lastSuccessfulSnapshot != null)
            {
                SetNotifyTooltip(BuildNativeTooltipWithStatus(refreshingSnapshot));
            }
            else
            {
                SetNotifyTooltip("Codex Usage: refreshing");
            }
            UpdateUsagePopup(refreshingSnapshot);
            SetUsagePopupRefreshing(true);
        }

        private void ApplySuccessfulSnapshot(UsageSnapshot snapshot, bool showBalloon)
        {
            if (snapshot.LastUpdated == DateTime.MinValue)
            {
                snapshot.LastUpdated = DateTime.Now;
            }
            snapshot.LastAttempted = lastAttemptedAt;
            snapshot.StatusMessage = "";
            snapshot.IsStale = false;
            snapshot.IsRefreshing = false;
            snapshot.IsPaused = false;

            UsageResetKind resets = usageResetDetector.Observe(snapshot);
            lastSuccessfulSnapshot = snapshot.Clone();
            currentSnapshot = snapshot.Clone();
            RenderDataSnapshot(
                currentSnapshot,
                true,
                true,
                showBalloon && resets == UsageResetKind.None);
            ShowUsageResetNotification(resets);
        }

        private void ApplyRefreshFailure(UsageSnapshot failure, bool showBalloon)
        {
            string failureMessage = failure != null && !string.IsNullOrWhiteSpace(failure.ErrorMessage)
                ? failure.ErrorMessage
                : RefreshFailedWithoutDataStatus;
            if (lastSuccessfulSnapshot != null)
            {
                UsageSnapshot staleSnapshot = lastSuccessfulSnapshot.Clone();
                staleSnapshot.LastAttempted = lastAttemptedAt;
                staleSnapshot.ErrorMessage = failureMessage;
                staleSnapshot.StatusMessage = failureMessage;
                staleSnapshot.IsStale = true;
                staleSnapshot.IsRefreshing = false;
                staleSnapshot.IsPaused = false;
                currentSnapshot = staleSnapshot;

                SetNotifyTooltip(BuildNativeTooltipWithStatus(staleSnapshot));
                UpdateUsagePopup(staleSnapshot);
            }
            else
            {
                UsageSnapshot failedSnapshot = failure != null
                    ? failure.Clone()
                    : UsageSnapshot.FromError(failureMessage);
                failedSnapshot.LastUpdated = DateTime.MinValue;
                failedSnapshot.LastAttempted = lastAttemptedAt;
                failedSnapshot.ErrorMessage = failureMessage;
                failedSnapshot.StatusMessage = failureMessage;
                failedSnapshot.IsStale = true;
                failedSnapshot.IsRefreshing = false;
                failedSnapshot.IsPaused = false;
                currentSnapshot = failedSnapshot;

                SetTrayIcon(IconRenderer.CreateErrorIcon());
                SetNotifyTooltip("Codex Usage: " + failureMessage);
                UpdateUsagePopup(failedSnapshot);
            }

            if (showBalloon)
            {
                notifyIcon.ShowBalloonTip(3000, "Codex Usage", failureMessage, ToolTipIcon.Warning);
            }
        }

        private static string BuildPausedFailureStatus(string failureMessage)
        {
            if (string.IsNullOrWhiteSpace(failureMessage))
            {
                return PausedAfterFailureStatus;
            }

            return "Checks paused - " + failureMessage;
        }

        private static string GetFailureStatus(UsageSnapshot snapshot, string fallback)
        {
            return snapshot != null && !string.IsNullOrWhiteSpace(snapshot.ErrorMessage)
                ? snapshot.ErrorMessage
                : fallback;
        }

        private void ApplyPausedState()
        {
            refreshTimer.Stop();
            bool failedLastAttempt = currentSnapshot != null && currentSnapshot.IsStale;
            string failureMessage = failedLastAttempt && currentSnapshot != null
                ? currentSnapshot.ErrorMessage
                : null;

            if (lastSuccessfulSnapshot != null)
            {
                UsageSnapshot pausedSnapshot = lastSuccessfulSnapshot.Clone();
                pausedSnapshot.LastAttempted = lastAttemptedAt;
                pausedSnapshot.ErrorMessage = failureMessage;
                pausedSnapshot.StatusMessage = failedLastAttempt
                    ? BuildPausedFailureStatus(failureMessage)
                    : PausedStatus;
                pausedSnapshot.IsStale = failedLastAttempt;
                pausedSnapshot.IsRefreshing = refreshInProgress;
                pausedSnapshot.IsPaused = true;
                currentSnapshot = pausedSnapshot;

                SetNotifyTooltip(BuildNativeTooltipWithStatus(pausedSnapshot));
                UpdateUsagePopup(pausedSnapshot);
            }
            else
            {
                UsageSnapshot waitingSnapshot = failedLastAttempt
                    ? currentSnapshot.Clone()
                    : UsageSnapshot.FromError(WaitingStatus);
                waitingSnapshot.LastUpdated = DateTime.MinValue;
                waitingSnapshot.LastAttempted = lastAttemptedAt;
                waitingSnapshot.ErrorMessage = failureMessage;
                waitingSnapshot.StatusMessage = failedLastAttempt
                    ? BuildPausedFailureStatus(failureMessage)
                    : WaitingStatus;
                waitingSnapshot.IsStale = failedLastAttempt;
                waitingSnapshot.IsRefreshing = refreshInProgress;
                waitingSnapshot.IsPaused = true;
                currentSnapshot = waitingSnapshot;

                SetTrayIcon(IconRenderer.CreateUnknownIcon("..."));
                SetNotifyTooltip("Codex Usage: waiting for Codex");
                UpdateUsagePopup(waitingSnapshot);
            }
            SetUsagePopupRefreshing(refreshInProgress);
        }

        private void RestoreActiveState()
        {
            if (currentSnapshot == null || !currentSnapshot.IsPaused)
            {
                return;
            }

            if (lastSuccessfulSnapshot != null)
            {
                UsageSnapshot activeSnapshot = currentSnapshot.IsStale
                    ? currentSnapshot.Clone()
                    : lastSuccessfulSnapshot.Clone();
                activeSnapshot.IsPaused = false;
                activeSnapshot.IsRefreshing = refreshInProgress;
                activeSnapshot.StatusMessage = refreshInProgress
                    ? RefreshingStatus
                    : (activeSnapshot.IsStale
                        ? GetFailureStatus(activeSnapshot, RefreshFailedStatus)
                        : "");
                currentSnapshot = activeSnapshot;
                RenderDataSnapshot(currentSnapshot, false, false, false);
            }
            else
            {
                UsageSnapshot activeSnapshot = currentSnapshot.Clone();
                activeSnapshot.IsPaused = false;
                activeSnapshot.IsRefreshing = refreshInProgress;
                activeSnapshot.StatusMessage = refreshInProgress
                    ? RefreshingStatus
                    : (activeSnapshot.IsStale
                        ? GetFailureStatus(activeSnapshot, RefreshFailedWithoutDataStatus)
                        : "Checking limits...");
                currentSnapshot = activeSnapshot;
                UpdateUsagePopup(currentSnapshot);
                SetUsagePopupRefreshing(refreshInProgress);
            }
        }

        private void RenderDataSnapshot(UsageSnapshot snapshot, bool renderIcon, bool updateThresholds, bool showBalloon)
        {
            if (snapshot == null)
            {
                return;
            }

            LimitWindow iconWindow = GetIconWindow(snapshot);
            if (renderIcon)
            {
                if (iconWindow == null)
                {
                    SetTrayIcon(IconRenderer.CreateErrorIcon());
                }
                else
                {
                    int remaining = ClampPercent(100.0 - iconWindow.UsedPercent);
                    SetTrayIcon(IconRenderer.CreatePercentIcon(remaining, settings));
                }
            }

            SetNotifyTooltip(BuildNativeTooltipWithStatus(snapshot));
            UpdateUsagePopup(snapshot);
            bool thresholdBalloonShown = updateThresholds && UpdateThresholdNotifications(snapshot);

            if (showBalloon && !thresholdBalloonShown)
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
            if (snapshot == null)
            {
                return "Usage unavailable";
            }

            List<string> windows = new List<string>();
            if (snapshot.Weekly != null)
            {
                windows.Add("Weekly " + FormatWindow(snapshot.Weekly));
            }
            if (snapshot.FiveHour != null)
            {
                windows.Add("5h " + FormatWindow(snapshot.FiveHour));
            }
            return windows.Count > 0
                ? string.Join(" | ", windows.ToArray())
                : "Usage unavailable";
        }

        private static string BuildNativeTooltip(UsageSnapshot snapshot)
        {
            StringBuilder tooltip = new StringBuilder("Codex Usage Remaining");
            if (snapshot != null && snapshot.Weekly != null)
            {
                tooltip.AppendLine();
                tooltip.Append("Weekly: ");
                tooltip.Append(FormatNativeWindow(snapshot.Weekly));
            }
            if (snapshot != null && snapshot.FiveHour != null)
            {
                tooltip.AppendLine();
                tooltip.Append("5h: ");
                tooltip.Append(FormatNativeWindow(snapshot.FiveHour));
            }
            if (snapshot == null || !snapshot.HasPrimaryLimit)
            {
                tooltip.AppendLine();
                tooltip.Append("Usage unavailable");
            }
            return tooltip.ToString();
        }

        private static string BuildNativeTooltipWithStatus(UsageSnapshot snapshot)
        {
            string tooltip = BuildNativeTooltip(snapshot);
            if (snapshot != null && !string.IsNullOrWhiteSpace(snapshot.StatusMessage))
            {
                tooltip += Environment.NewLine + snapshot.StatusMessage;
            }
            return tooltip;
        }

        private static string FormatWindow(LimitWindow window)
        {
            if (window == null)
            {
                return "unavailable";
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
                return "unavailable";
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

        private void UpdateUsagePopup(UsageSnapshot snapshot)
        {
            UsagePopup popup = usagePopup;
            if (popup != null && !popup.IsDisposed && !popup.Disposing)
            {
                popup.UpdateSnapshot(snapshot);
            }
        }

        private void SetUsagePopupRefreshing(bool refreshing)
        {
            UsagePopup popup = usagePopup;
            if (popup != null && !popup.IsDisposed && !popup.Disposing)
            {
                popup.SetRefreshing(refreshing);
            }
        }

        private bool TryPostToUi(Action action)
        {
            if (shuttingDown || action == null || dispatcher.IsDisposed || !dispatcher.IsHandleCreated)
            {
                return false;
            }

            try
            {
                dispatcher.BeginInvoke(new Action(delegate
                {
                    if (!shuttingDown)
                    {
                        action();
                    }
                }));
                return true;
            }
            catch (ObjectDisposedException)
            {
                return false;
            }
            catch (InvalidOperationException)
            {
                return false;
            }
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

            bool preferFiveHour = string.Equals(
                settings.IconMetric,
                AppSettings.IconMetricFiveHour,
                StringComparison.OrdinalIgnoreCase);
            LimitWindow preferred = preferFiveHour ? snapshot.FiveHour : snapshot.Weekly;
            LimitWindow fallback = preferFiveHour ? snapshot.Weekly : snapshot.FiveHour;
            return preferred ?? fallback;
        }

        private bool UpdateThresholdNotifications(UsageSnapshot snapshot)
        {
            int weeklyLevel = GetThresholdLevel(snapshot != null ? snapshot.Weekly : null);
            int fiveHourLevel = GetThresholdLevel(snapshot != null ? snapshot.FiveHour : null);

            bool shouldNotify = settings.ThresholdNotifications;
            bool weeklyAlert = shouldNotify && IsNewThresholdLevel(lastWeeklyThresholdLevel, weeklyLevel);
            bool fiveHourAlert = shouldNotify && IsNewThresholdLevel(lastFiveHourThresholdLevel, fiveHourLevel);

            lastWeeklyThresholdLevel = weeklyLevel;
            lastFiveHourThresholdLevel = fiveHourLevel;

            if (!weeklyAlert && !fiveHourAlert)
            {
                return false;
            }

            StringBuilder message = new StringBuilder();
            int highestLevel = 0;
            if (weeklyAlert)
            {
                AppendThresholdLine(message, "Weekly", snapshot.Weekly, weeklyLevel);
                highestLevel = Math.Max(highestLevel, weeklyLevel);
            }
            if (fiveHourAlert)
            {
                AppendThresholdLine(message, "5h", snapshot.FiveHour, fiveHourLevel);
                highestLevel = Math.Max(highestLevel, fiveHourLevel);
            }

            notifyIcon.ShowBalloonTip(
                5000,
                highestLevel >= 2 ? "Codex Usage Critical" : "Codex Usage Low",
                message.ToString(),
                ToolTipIcon.Warning);
            return true;
        }

        private void ShowUsageResetNotification(UsageResetKind resets)
        {
            if (resets == UsageResetKind.None)
            {
                return;
            }

            bool weekly = (resets & UsageResetKind.Weekly) != 0;
            bool fiveHour = (resets & UsageResetKind.FiveHour) != 0;
            string message;
            if (weekly && fiveHour)
            {
                message = "Weekly and 5-hour usage are back to 100%.";
            }
            else if (weekly)
            {
                message = "Weekly usage is back to 100%.";
            }
            else
            {
                message = "5-hour usage is back to 100%.";
            }

            notifyIcon.ShowBalloonTip(
                5000,
                "Codex Usage Reset",
                message,
                ToolTipIcon.Info);
        }

        private int GetThresholdLevel(LimitWindow window)
        {
            if (window == null)
            {
                return 0;
            }

            int remaining = window.RemainingPercent;
            if (remaining <= settings.CriticalThreshold)
            {
                return 2;
            }
            if (remaining <= settings.LowThreshold)
            {
                return 1;
            }

            return 0;
        }

        private static bool IsNewThresholdLevel(int? previousLevel, int currentLevel)
        {
            return previousLevel.HasValue && currentLevel > previousLevel.Value;
        }

        private static void AppendThresholdLine(StringBuilder message, string label, LimitWindow window, int level)
        {
            if (message.Length > 0)
            {
                message.AppendLine();
            }

            string severity = level >= 2 ? "critical" : "low";
            int remaining = window != null ? window.RemainingPercent : 0;
            message.Append(label);
            message.Append(" usage is ");
            message.Append(severity);
            message.Append(": ");
            message.Append(remaining);
            message.Append("% remaining.");
        }

        private void ResetThresholdNotificationState()
        {
            lastWeeklyThresholdLevel = null;
            lastFiveHourThresholdLevel = null;
        }

        private void ToggleUsagePopup()
        {
            UsagePopup popup = usagePopup;
            if (popup != null && !popup.IsDisposed && popup.Visible)
            {
                popup.Hide();
            }
            else
            {
                ShowUsagePopup();
            }
        }

        private void ShowUsagePopup()
        {
            UsagePopup popup = EnsureUsagePopup();
            if (popup == null)
            {
                return;
            }

            UsageSnapshot snapshot = currentSnapshot;
            if (snapshot == null)
            {
                snapshot = UsageSnapshot.FromError("Checking limits...");
                snapshot.LastUpdated = DateTime.MinValue;
                snapshot.LastAttempted = lastAttemptedAt;
                snapshot.StatusMessage = "Checking limits...";
                snapshot.IsRefreshing = refreshInProgress;
            }

            popup.UpdateSnapshot(snapshot);
            popup.SetRefreshing(refreshInProgress);
            popup.ShowNear(Cursor.Position);
        }

        private void ShowSettings()
        {
            if (settingsForm != null && !settingsForm.IsDisposed)
            {
                ShowAndActivateSettingsForm();
                return;
            }

            settingsForm = new SettingsForm(settings);
            settingsForm.SetUpdateStatus(updateStatusText);
            settingsForm.SettingsApplied += delegate
            {
                ApplyStartupSetting();
                UsagePopup popup = usagePopup;
                if (popup != null && !popup.IsDisposed && !popup.Disposing)
                {
                    popup.ApplySettings(settings);
                }
                ResetThresholdNotificationState();
                if (currentSnapshot != null && lastSuccessfulSnapshot != null)
                {
                    RenderDataSnapshot(currentSnapshot, true, true, false);
                }
                RefreshScheduleChanged();
            };
            settingsForm.CheckUpdatesRequested += delegate { CheckForUpdates(true); };
            settingsForm.Show();
            ShowAndActivateSettingsForm();
        }

        private void ShowAndActivateSettingsForm()
        {
            if (settingsForm == null || settingsForm.IsDisposed)
            {
                return;
            }

            if (!settingsForm.Visible)
            {
                settingsForm.Show();
            }
            if (settingsForm.WindowState == FormWindowState.Minimized)
            {
                settingsForm.WindowState = FormWindowState.Normal;
            }

            IntPtr windowHandle = settingsForm.Handle;
            ShowWindow(windowHandle, ShowWindowRestore);
            settingsForm.BringToFront();
            settingsForm.Activate();
            SetForegroundWindow(windowHandle);
        }

        private void ApplyStartupSetting()
        {
            try
            {
                StartupManager.SetEnabled(settings.StartWithWindows);
            }
            catch (Exception ex)
            {
                settings.StartWithWindows = StartupManager.IsEnabled();
                string message = "Could not update Windows startup setting:" +
                    Environment.NewLine + Environment.NewLine + ex.Message;
                try
                {
                    settings.Save();
                }
                catch (Exception saveException)
                {
                    message += Environment.NewLine + Environment.NewLine +
                        "The corrected setting could not be saved: " + saveException.Message;
                }
                MessageBox.Show(
                    GetSettingsOwner(),
                    message,
                    "Codex Usage Settings",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Warning);
            }
        }

        private UsagePopup CreateUsagePopup()
        {
            UsagePopup popup = new UsagePopup(settings);
            popup.RefreshRequested += delegate { RefreshUsage(false); };
            popup.SettingsRequested += delegate(object sender, EventArgs e)
            {
                UsagePopup source = sender as UsagePopup;
                if (source != null && !source.IsDisposed)
                {
                    source.Hide();
                }
                ShowSettings();
            };
            return popup;
        }

        private UsagePopup EnsureUsagePopup()
        {
            if (shuttingDown)
            {
                return null;
            }

            if (usagePopup == null || usagePopup.IsDisposed)
            {
                usagePopup = CreateUsagePopup();
            }
            return usagePopup;
        }

        private void CheckForUpdates(bool interactive)
        {
            if (shuttingDown)
            {
                return;
            }

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
            SetUpdateStatusText("Checking for updates...");

            CancellationToken cancellationToken = shutdownCancellation.Token;
            Task.Factory.StartNew(
                delegate
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    return UpdateService.CheckForUpdate();
                },
                cancellationToken,
                TaskCreationOptions.None,
                TaskScheduler.Default).ContinueWith(delegate(Task<UpdateInfo> task)
            {
                if (task.IsFaulted)
                {
                    AggregateException ignored = task.Exception;
                }

                TryPostToUi(delegate
                {
                    updateCheckInProgress = false;
                    SetSettingsUpdateButtonState(false, null);
                    if (task.IsCanceled)
                    {
                        SetUpdateStatusText("Update check canceled.");
                    }
                    else if (task.IsFaulted)
                    {
                        HandleUpdateError(task.Exception, interactive);
                    }
                    else
                    {
                        HandleUpdateInfo(task.Result, interactive);
                    }
                });
            }, CancellationToken.None, TaskContinuationOptions.None, TaskScheduler.Default);
        }

        private void HandleUpdateInfo(UpdateInfo info, bool interactive)
        {
            if (info == null)
            {
                SetUpdateStatusText("Could not read update information.");
                if (interactive)
                {
                    MessageBox.Show(GetSettingsOwner(), "Could not read update information.", "Codex Usage Updates", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                }
                return;
            }

            if (!info.UpdateAvailable)
            {
                SetUpdateStatusText("Up to date: " + info.LatestVersion);
                if (interactive)
                {
                    MessageBox.Show(GetSettingsOwner(), info.Message, "Codex Usage Updates", MessageBoxButtons.OK, MessageBoxIcon.Information);
                }
                return;
            }

            SetUpdateStatusText("Version " + info.LatestVersion + " available");

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
            if (shuttingDown || updateInstallInProgress)
            {
                return;
            }

            updateInstallInProgress = true;
            SetSettingsUpdateButtonState(true, "Installing...");
            SetUpdateStatusText("Installing version " + info.LatestVersion + "...");

            CancellationToken cancellationToken = shutdownCancellation.Token;
            Task.Factory.StartNew(
                delegate
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    return UpdateService.InstallUpdate(info);
                },
                cancellationToken,
                TaskCreationOptions.None,
                TaskScheduler.Default).ContinueWith(delegate(Task<string> task)
            {
                if (task.IsFaulted)
                {
                    AggregateException ignored = task.Exception;
                }

                TryPostToUi(delegate
                {
                    if (task.IsCanceled)
                    {
                        updateInstallInProgress = false;
                        SetSettingsUpdateButtonState(false, null);
                        SetUpdateStatusText("Update installation canceled.");
                        return;
                    }
                    if (task.IsFaulted)
                    {
                        updateInstallInProgress = false;
                        SetSettingsUpdateButtonState(false, null);
                        HandleUpdateError(task.Exception, true);
                        return;
                    }

                    notifyIcon.ShowBalloonTip(2000, "Codex Usage Updates", task.Result, ToolTipIcon.Info);
                    ExitThread();
                });
            }, CancellationToken.None, TaskContinuationOptions.None, TaskScheduler.Default);
        }

        private void HandleUpdateError(AggregateException exception, bool interactive)
        {
            Exception baseException = exception != null ? exception.GetBaseException() : null;
            string message = baseException != null ? baseException.Message : "Unknown update error.";
            SetUpdateStatusText("Update check failed.");

            if (!interactive)
            {
                return;
            }

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

        private void SetUpdateStatusText(string status)
        {
            updateStatusText = status ?? "";
            if (settingsForm != null && !settingsForm.IsDisposed)
            {
                settingsForm.SetUpdateStatus(updateStatusText);
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
            if (shuttingDown)
            {
                return;
            }

            shuttingDown = true;
            try
            {
                shutdownCancellation.Cancel();
            }
            catch (ObjectDisposedException)
            {
            }

            refreshTimer.Stop();
            refreshTimer.Dispose();
            activityTimer.Stop();
            activityTimer.Dispose();
            if (startupUiTimer != null)
            {
                startupUiTimer.Stop();
                startupUiTimer.Dispose();
                startupUiTimer = null;
            }
            if (usagePopup != null && !usagePopup.IsDisposed)
            {
                usagePopup.Dispose();
                usagePopup = null;
            }
            if (settingsForm != null && !settingsForm.IsDisposed)
            {
                settingsForm.Dispose();
                settingsForm = null;
            }
            notifyIcon.Visible = false;
            notifyIcon.Dispose();
            if (currentIcon != null)
            {
                currentIcon.Dispose();
                currentIcon = null;
            }
            dispatcher.Dispose();
            refreshClock.Stop();
            shutdownCancellation.Dispose();
            base.ExitThreadCore();
        }
    }
}
