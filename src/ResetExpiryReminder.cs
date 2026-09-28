using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;
using System.Web.Script.Serialization;

namespace CodexUsageTray
{
    internal sealed class ResetExpiryReminder
    {
        private readonly string path;
        private readonly Dictionary<string, DateTime> notified = new Dictionary<string, DateTime>();

        public ResetExpiryReminder(string path)
        {
            this.path = path;
            try
            {
                if (File.Exists(path) && new FileInfo(path).Length <= 1048576)
                {
                    Dictionary<string, DateTime> saved = new JavaScriptSerializer()
                        .Deserialize<Dictionary<string, DateTime>>(File.ReadAllText(path));
                    if (saved != null && saved.Count <= 2048)
                    {
                        foreach (KeyValuePair<string, DateTime> item in saved)
                        {
                            notified[item.Key] = ToUtc(item.Value);
                        }
                    }
                }
            }
            catch
            {
                // A missing or damaged reminder cache must not stop usage monitoring.
            }
        }

        public static string DefaultPath
        {
            get
            {
                return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                    "CodexUsageTray", "reset-reminders.json");
            }
        }

        public List<RateLimitResetCredit> Observe(UsageSnapshot snapshot, DateTime nowUtc, int leadHours)
        {
            List<RateLimitResetCredit> due = new List<RateLimitResetCredit>();
            if (snapshot == null || snapshot.IsStale || snapshot.IsRefreshing || snapshot.IsPaused ||
                !string.IsNullOrEmpty(snapshot.ErrorMessage) || snapshot.AvailableResets == null)
            {
                return due;
            }

            nowUtc = ToUtc(nowUtc);
            List<string> expired = new List<string>();
            foreach (KeyValuePair<string, DateTime> item in notified)
            {
                if (item.Value <= nowUtc) expired.Add(item.Key);
            }
            foreach (string key in expired) notified.Remove(key);

            DateTime deadline = nowUtc.AddHours(Math.Max(1, Math.Min(72, leadHours)));
            foreach (RateLimitResetCredit credit in snapshot.AvailableResets)
            {
                if (credit == null || !credit.ExpiresAtUtc.HasValue) continue;
                DateTime expiration = ToUtc(credit.ExpiresAtUtc.Value);
                string key = GetKey(credit);
                if (expiration <= nowUtc || expiration > deadline || notified.ContainsKey(key)) continue;
                if (notified.Count >= 2048) break;
                notified[key] = expiration;
                due.Add(credit);
            }
            if (due.Count > 0 || expired.Count > 0) Save();
            return due;
        }

        public DateTime? GetNextCheckUtc(UsageSnapshot snapshot, DateTime nowUtc, int leadHours)
        {
            if (snapshot == null || snapshot.AvailableResets == null) return null;
            nowUtc = ToUtc(nowUtc);
            DateTime? next = null;
            foreach (RateLimitResetCredit credit in snapshot.AvailableResets)
            {
                if (credit == null || !credit.ExpiresAtUtc.HasValue || notified.ContainsKey(GetKey(credit))) continue;
                DateTime expiration = ToUtc(credit.ExpiresAtUtc.Value);
                if (expiration <= nowUtc) continue;
                DateTime check = expiration.AddHours(-Math.Max(1, Math.Min(72, leadHours)));
                if (check < nowUtc) check = nowUtc;
                if (!next.HasValue || check < next.Value) next = check;
            }
            return next;
        }

        private static string GetKey(RateLimitResetCredit credit)
        {
            return (credit.Id ?? (credit.ResetType ?? "") + ":" + (credit.Title ?? "")) + ":" +
                ToUtc(credit.ExpiresAtUtc.Value).Ticks.ToString(CultureInfo.InvariantCulture);
        }

        private static DateTime ToUtc(DateTime value)
        {
            return value.Kind == DateTimeKind.Unspecified
                ? DateTime.SpecifyKind(value, DateTimeKind.Utc) : value.ToUniversalTime();
        }

        private void Save()
        {
            string tempPath = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            try
            {
                string directory = Path.GetDirectoryName(path);
                if (string.IsNullOrEmpty(directory)) return;
                Directory.CreateDirectory(directory);
                File.WriteAllText(tempPath, new JavaScriptSerializer().Serialize(notified), new UTF8Encoding(false));
                if (File.Exists(path)) File.Replace(tempPath, path, null, true);
                else File.Move(tempPath, path);
            }
            catch
            {
                // In-memory deduplication still applies when the cache cannot be written.
            }
            finally
            {
                try { if (File.Exists(tempPath)) File.Delete(tempPath); }
                catch { }
            }
        }
    }
}
