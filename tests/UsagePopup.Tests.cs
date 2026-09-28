using System;
using System.IO;
using System.Reflection;
using System.Windows.Forms;

namespace CodexUsageTray.Tests
{
    internal static class UsagePopupTests
    {
        public static void Run()
        {
            UsageHistoryStore history = new UsageHistoryStore(Path.Combine(Path.GetTempPath(),
                "PopupTests-" + Guid.NewGuid().ToString("N") + ".json"));
            using (UsagePopup popup = new UsagePopup(new AppSettings(), history))
            {
                UsageSnapshot snapshot = new UsageSnapshot
                {
                    LastUpdated = DateTime.Now,
                    Weekly = new LimitWindow { UsedPercent = 25, WindowMinutes = 10080, ResetAfterSeconds = 86400 }
                };
                popup.UpdateSnapshot(snapshot);
                popup.UpdateClaudeSnapshot(snapshot.Clone(), history);
                Button refresh = Field<Button>(popup, "refreshButton");
                Button provider = Field<Button>(popup, "providerButton");
                Timer animation = Field<Timer>(popup, "refreshAnimationTimer");

                popup.SetClaudeRefreshing(true);
                Assert(refresh.Enabled && !animation.Enabled, "Claude refresh does not animate the Codex view");
                Click(provider);
                Assert(popup.IsShowingClaude && !refresh.Enabled && animation.Enabled &&
                    refresh.AccessibleName == "Refreshing usage", "Claude view starts its spinner and disables duplicate clicks");
                int before = Field<int>(popup, "refreshAnimationAngle");
                typeof(UsagePopup).GetMethod("RefreshAnimationTimer_Tick", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, new object[] { null, EventArgs.Empty });
                Assert(Field<int>(popup, "refreshAnimationAngle") != before, "refresh spinner advances");

                popup.SetRefreshing(true);
                popup.SetClaudeRefreshing(false);
                Assert(refresh.Enabled && !animation.Enabled && refresh.AccessibleName == "Refresh Claude account usage",
                    "Claude completion stops its spinner independently of Codex refresh");
                snapshot.StatusMessage = "Claude usage updated";
                popup.UpdateClaudeSnapshot(snapshot, history);
                Assert((string)typeof(UsagePopup).GetMethod("BuildStatusLine", BindingFlags.NonPublic | BindingFlags.Instance)
                    .Invoke(popup, null) == "Claude usage updated", "successful manual refresh has visible confirmation");
                Click(provider);
                Assert(!refresh.Enabled && animation.Enabled, "switching back preserves the active Codex refresh");
                popup.SetRefreshing(false);
                Assert(refresh.Enabled && !animation.Enabled, "all animations stop when their refresh completes");
            }
        }

        private static T Field<T>(object instance, string name)
        {
            return (T)instance.GetType().GetField(name, BindingFlags.NonPublic | BindingFlags.Instance).GetValue(instance);
        }
        private static void Click(Button button)
        {
            typeof(Button).GetMethod("OnClick", BindingFlags.NonPublic | BindingFlags.Instance)
                .Invoke(button, new object[] { EventArgs.Empty });
        }
        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException("Usage popup: " + message);
        }
    }
}
