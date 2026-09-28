using System;
using System.IO;
using System.Reflection;

namespace CodexUsageTray.Tests
{
    internal static class UsageFeaturesTests
    {
        public static void Run()
        {
            TestTrayMetric();
            string directory = Path.Combine(Path.GetTempPath(), "CodexUsageFeatures-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(directory);
            try
            {
                TestReminders(Path.Combine(directory, "reminders.json"));
                TestSettings(Path.Combine(directory, "settings.json"));
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
            settings.IconMetric = AppSettings.IconMetricAuto;
            settings.ResetExpiryReminders = false;
            settings.ResetExpiryLeadHours = 6;
            save.Invoke(settings, new object[] { path });
            AppSettings reloaded = (AppSettings)load.Invoke(null, new object[] { path });
            Assert(reloaded.IconMetric == AppSettings.IconMetricAuto && !reloaded.ResetExpiryReminders &&
                reloaded.ResetExpiryLeadHours == 6, "new settings round trip");
            AppSettings copy = new AppSettings();
            copy.CopyValuesFrom(reloaded);
            Assert(copy.IconMetric == AppSettings.IconMetricAuto && !copy.ResetExpiryReminders &&
                copy.ResetExpiryLeadHours == 6, "applying settings copies new values");
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException(message);
        }
    }
}
