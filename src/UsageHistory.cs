using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;
using System.Web.Script.Serialization;

namespace CodexUsageTray
{
    internal sealed class UsageHistoryWindow
    {
        public double UsedPercent;
        public DateTime? ResetAtUtc;
        public int? WindowMinutes;

        public UsageHistoryWindow() { }

        public UsageHistoryWindow Clone()
        {
            return (UsageHistoryWindow)MemberwiseClone();
        }
    }

    internal sealed class UsageHistorySample
    {
        public DateTime TimestampUtc;
        public UsageHistoryWindow Weekly;
        public UsageHistoryWindow FiveHour;

        public UsageHistorySample() { }

        public UsageHistorySample Clone()
        {
            return new UsageHistorySample
            {
                TimestampUtc = TimestampUtc,
                Weekly = Weekly == null ? null : Weekly.Clone(),
                FiveHour = FiveHour == null ? null : FiveHour.Clone()
            };
        }
    }

    internal sealed class UsageProjection
    {
        public DateTime StartUtc;
        public DateTime EndUtc;
        public double StartRemainingPercent;
        public double EndRemainingPercent;
        public bool EndsAtReset;
    }

    internal sealed class UsageHistoryStore
    {
        private const int MaxSamples = 45000;
        private const int MaxFileBytes = 16 * 1024 * 1024;
        private readonly string path;
        private readonly List<UsageHistorySample> samples = new List<UsageHistorySample>();
        public bool ImportCompleted { get; private set; }

        public UsageHistoryStore(string path)
        {
            this.path = path;
            bool loaded = Load();
            ImportCompleted = loaded && File.Exists(path + ".imported");
        }

        public static string DefaultPath
        {
            get
            {
                return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                    "CodexUsageTray", "usage-history.json");
            }
        }

        public List<UsageHistorySample> Samples
        {
            get
            {
                List<UsageHistorySample> copy = new List<UsageHistorySample>();
                foreach (UsageHistorySample sample in samples) copy.Add(sample.Clone());
                return copy;
            }
        }

        public bool Observe(UsageSnapshot snapshot)
        {
            if (!IsValidSnapshot(snapshot) || snapshot.IsRefreshing || snapshot.IsPaused) return false;
            DateTime timestamp = snapshot.LastUpdated.ToUniversalTime();
            DateTime now = DateTime.UtcNow;
            if (timestamp < now.AddDays(-14) || timestamp > now.AddMinutes(1) ||
                (samples.Count > 0 && timestamp <= samples[samples.Count - 1].TimestampUtc)) return false;

            samples.Add(new UsageHistorySample
            {
                TimestampUtc = timestamp,
                Weekly = FromLimit(snapshot.Weekly, timestamp),
                FiveHour = FromLimit(snapshot.FiveHour, timestamp)
            });
            Prune(now);
            Save();
            return true;
        }

        public bool Clear()
        {
            samples.Clear();
            bool saved = Save();
            MarkImportCompleted(saved);
            return saved;
        }

        public int Import(IEnumerable<UsageSnapshot> snapshots)
        {
            if (snapshots == null || ImportCompleted) return 0;
            DateTime now = DateTime.UtcNow;
            SortedDictionary<DateTime, UsageHistorySample> merged = new SortedDictionary<DateTime, UsageHistorySample>();
            foreach (UsageHistorySample sample in samples) merged[sample.TimestampUtc] = sample;
            int added = 0;
            foreach (UsageSnapshot snapshot in snapshots)
            {
                if (!IsValidSnapshot(snapshot) || snapshot.IsPaused || snapshot.IsRefreshing) continue;
                DateTime timestamp = snapshot.LastUpdated.ToUniversalTime();
                if (timestamp < now.AddDays(-14) || timestamp > now) continue;
                UsageHistorySample existing;
                if (!merged.TryGetValue(timestamp, out existing))
                {
                    merged[timestamp] = new UsageHistorySample
                    {
                        TimestampUtc = timestamp,
                        Weekly = FromLimit(snapshot.Weekly, timestamp),
                        FiveHour = FromLimit(snapshot.FiveHour, timestamp)
                    };
                    added++;
                }
                else
                {
                    // Live observations take precedence; imports can fill an absent window.
                    if (existing.Weekly == null) existing.Weekly = FromLimit(snapshot.Weekly, timestamp);
                    if (existing.FiveHour == null) existing.FiveHour = FromLimit(snapshot.FiveHour, timestamp);
                }
            }
            samples.Clear();
            samples.AddRange(merged.Values);
            Prune(now);
            MarkImportCompleted(Save());
            return added;
        }

        private void MarkImportCompleted(bool persist)
        {
            ImportCompleted = true;
            if (!persist) return;
            try
            {
                Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path)));
                File.WriteAllText(path + ".imported", "1");
            }
            catch (Exception) { }
        }

        public static UsageHistoryWindow GetWindow(UsageHistorySample sample, bool weekly)
        {
            return sample == null ? null : (weekly ? sample.Weekly : sample.FiveHour);
        }

        public static bool IsReset(UsageHistorySample previous, UsageHistorySample current, bool weekly)
        {
            UsageHistoryWindow before = GetWindow(previous, weekly);
            UsageHistoryWindow after = GetWindow(current, weekly);
            if (before == null || after == null) return false;
            return after.UsedPercent < before.UsedPercent - 0.001 ||
                (before.ResetAtUtc.HasValue && current.TimestampUtc >= before.ResetAtUtc.Value) ||
                (before.ResetAtUtc.HasValue && after.ResetAtUtc.HasValue &&
                    Math.Abs((after.ResetAtUtc.Value - before.ResetAtUtc.Value).TotalMinutes) > 2);
        }

        public static bool IsContinuous(UsageHistorySample previous, UsageHistorySample current, bool weekly)
        {
            if (!SameCycle(previous, current, weekly)) return false;
            return (current.TimestampUtc - previous.TimestampUtc).TotalHours <= 2;
        }

        public string GetForecast(bool weekly, UsageSnapshot snapshot, DateTime nowUtc)
        {
            nowUtc = nowUtc.ToUniversalTime();
            if (!IsFresh(snapshot, nowUtc)) return "Refresh usage for an estimate";
            LimitWindow limit = weekly ? snapshot.Weekly : snapshot.FiveHour;
            if (limit == null) return "Usage estimate unavailable";
            if (limit.UsedPercent >= 100) return "Usage limit reached";
            UsageHistoryWindow window = FromLimit(limit, snapshot.LastUpdated.ToUniversalTime());
            if (window.ResetAtUtc.HasValue && window.ResetAtUtc.Value <= nowUtc) return "Awaiting reset update";
            List<UsageHistorySample> periodSamples = samples.FindAll(
                delegate(UsageHistorySample sample) { return GetWindow(sample, weekly) != null; });
            double ratePerHour;
            if (!TryGetRecentRate(weekly, snapshot, periodSamples, out ratePerHour))
                return GetInitialForecast(weekly, snapshot, periodSamples);
            if (ratePerHour <= 0) return "No usage increase in recent history";
            double remainingHours = (100 - limit.UsedPercent) / ratePerHour;
            if (window.ResetAtUtc.HasValue && remainingHours >=
                (window.ResetAtUtc.Value - snapshot.LastUpdated.ToUniversalTime()).TotalHours)
                return "Will last until reset";
            return "\u2248" + FormatDuration(remainingHours) + " left at current pace";
        }

        public UsageProjection GetProjection(bool weekly, UsageSnapshot snapshot, DateTime nowUtc)
        {
            nowUtc = nowUtc.ToUniversalTime();
            if (!IsFresh(snapshot, nowUtc) || snapshot.IsRefreshing) return null;
            LimitWindow limit = weekly ? snapshot.Weekly : snapshot.FiveHour;
            if (limit == null || limit.UsedPercent >= 100 || !limit.ResetAfterSeconds.HasValue) return null;

            DateTime startUtc = snapshot.LastUpdated.ToUniversalTime();
            DateTime resetUtc = startUtc.AddSeconds(limit.ResetAfterSeconds.Value);
            if (resetUtc <= nowUtc) return null;

            List<UsageHistorySample> periodSamples = samples.FindAll(
                delegate(UsageHistorySample sample) { return GetWindow(sample, weekly) != null; });
            double ratePerHour;
            if (!TryGetRecentRate(weekly, snapshot, periodSamples, out ratePerHour) &&
                !TryGetCycleAverageRate(weekly, snapshot, periodSamples, out ratePerHour)) return null;

            double remaining = 100 - limit.UsedPercent;
            double hoursUntilReset = (resetUtc - startUtc).TotalHours;
            bool endsAtReset = ratePerHour <= 0 || remaining / ratePerHour >= hoursUntilReset;
            DateTime endUtc = endsAtReset ? resetUtc : startUtc.AddHours(remaining / ratePerHour);
            return new UsageProjection
            {
                StartUtc = startUtc,
                EndUtc = endUtc,
                StartRemainingPercent = remaining,
                EndRemainingPercent = endsAtReset ? Math.Max(0, remaining - ratePerHour * hoursUntilReset) : 0,
                EndsAtReset = endsAtReset
            };
        }

        private static bool TryGetRecentRate(bool weekly, UsageSnapshot snapshot,
            List<UsageHistorySample> periodSamples, out double ratePerHour)
        {
            ratePerHour = 0;
            if (periodSamples.Count < 3) return false;
            LimitWindow limit = weekly ? snapshot.Weekly : snapshot.FiveHour;
            int end = periodSamples.Count - 1;
            UsageHistorySample last = periodSamples[end];
            UsageHistoryWindow lastWindow = GetWindow(last, weekly);
            if (lastWindow == null || Math.Abs((last.TimestampUtc - snapshot.LastUpdated.ToUniversalTime()).TotalSeconds) > 1 ||
                Math.Abs(lastWindow.UsedPercent - limit.UsedPercent) > 0.001) return false;

            DateTime cutoff = last.TimestampUtc.AddHours(weekly ? -48 : -1);
            int start = end;
            while (start > 0 && periodSamples[start - 1].TimestampUtc >= cutoff)
            {
                UsageHistorySample before = periodSamples[start - 1];
                UsageHistorySample after = periodSamples[start];
                if (!SameCycle(before, after, weekly)) break;
                // Overnight observations still give a valid weekly wall-clock average
                // when a known reset deadline proves they belong to the same cycle.
                if (!IsContinuous(before, after, weekly) &&
                    (!weekly || !HasMatchingReset(before, after, weekly))) break;
                start--;
            }

            double hours = (last.TimestampUtc - periodSamples[start].TimestampUtc).TotalHours;
            if (end - start < 2 || hours < (weekly ? 6 : 0.25)) return false;
            // Weight elapsed time, not sample count: frequent polling must not amplify
            // a burst. Integrating the decay over each interval also keeps a steady
            // rate unchanged when the same interval is split into more observations.
            double decayPerHour = Math.Log(2) / (weekly ? 6 : 0.25);
            double weightedRate = 0;
            double totalWeight = 0;
            for (int i = start + 1; i <= end; i++)
            {
                UsageHistorySample before = periodSamples[i - 1];
                UsageHistorySample after = periodSamples[i];
                double intervalHours = (after.TimestampUtc - before.TimestampUtc).TotalHours;
                double weight = Math.Exp(-decayPerHour * (last.TimestampUtc - after.TimestampUtc).TotalHours) -
                    Math.Exp(-decayPerHour * (last.TimestampUtc - before.TimestampUtc).TotalHours);
                double increase = Math.Max(0, GetWindow(after, weekly).UsedPercent - GetWindow(before, weekly).UsedPercent);
                weightedRate += increase / intervalHours * weight;
                totalWeight += weight;
            }
            ratePerHour = weightedRate / totalWeight;
            return true;
        }

        private static string GetInitialForecast(bool weekly, UsageSnapshot snapshot,
            List<UsageHistorySample> periodSamples)
        {
            LimitWindow limit = weekly ? snapshot.Weekly : snapshot.FiveHour;
            double ratePerHour;
            if (!TryGetCycleAverageRate(weekly, snapshot, periodSamples, out ratePerHour))
                return "Learning usage pace...";
            if (ratePerHour <= 0) return "No usage yet this cycle";
            double remainingHours = (100 - limit.UsedPercent) / ratePerHour;
            DateTime reset = snapshot.LastUpdated.ToUniversalTime().AddSeconds(limit.ResetAfterSeconds.Value);
            if (remainingHours >= (reset - snapshot.LastUpdated.ToUniversalTime()).TotalHours)
                return "Will last until reset";
            return "\u2248" + FormatDuration(remainingHours) + " left at cycle average";
        }

        private static bool TryGetCycleAverageRate(bool weekly, UsageSnapshot snapshot,
            List<UsageHistorySample> periodSamples, out double ratePerHour)
        {
            ratePerHour = 0;
            LimitWindow limit = weekly ? snapshot.Weekly : snapshot.FiveHour;
            if (!limit.ResetAfterSeconds.HasValue || !limit.WindowMinutes.HasValue) return false;
            double duration = limit.WindowMinutes.Value * 60.0;
            double elapsed = duration - limit.ResetAfterSeconds.Value;
            // OpenQuota's pacing approach: avoid projecting the very start of a window.
            if (elapsed < Math.Max(60, duration * 0.01) || elapsed >= duration) return false;
            DateTime reset = snapshot.LastUpdated.ToUniversalTime().AddSeconds(limit.ResetAfterSeconds.Value);
            for (int i = periodSamples.Count - 1; i > 0; i--)
            {
                UsageHistoryWindow after = GetWindow(periodSamples[i], weekly);
                if (!after.ResetAtUtc.HasValue || Math.Abs((after.ResetAtUtc.Value - reset).TotalMinutes) > 2) break;
                // Use the same decrease tolerance as IsReset so measurement jitter
                // cannot discard a valid cycle-average forecast and chart projection.
                if (HasMatchingReset(periodSamples[i - 1], periodSamples[i], weekly) &&
                    after.UsedPercent < GetWindow(periodSamples[i - 1], weekly).UsedPercent - 0.001)
                    return false; // A banked reset invalidates the inferred cycle start.
            }
            ratePerHour = limit.UsedPercent * 3600 / elapsed;
            return true;
        }

        private static string FormatDuration(double hours)
        {
            if (hours < 1.0 / 12) return "<5 minutes";
            if (hours < 1) return (Math.Round(hours * 12) * 5).ToString("0", CultureInfo.InvariantCulture) + " minutes";
            double value = hours < 24 ? Math.Round(hours * 2) / 2 : Math.Round(hours / 24, 1, MidpointRounding.AwayFromZero);
            string unit = hours < 24 ? "hour" : "day";
            return value.ToString("0.#", CultureInfo.InvariantCulture) + " " + unit + (value == 1 ? "" : "s");
        }

        private static bool SameCycle(UsageHistorySample previous, UsageHistorySample current, bool weekly)
        {
            UsageHistoryWindow before = GetWindow(previous, weekly);
            UsageHistoryWindow after = GetWindow(current, weekly);
            return before != null && after != null && current.TimestampUtc > previous.TimestampUtc &&
                before.WindowMinutes == after.WindowMinutes && !IsReset(previous, current, weekly);
        }

        private static bool HasMatchingReset(UsageHistorySample previous, UsageHistorySample current, bool weekly)
        {
            UsageHistoryWindow before = GetWindow(previous, weekly);
            UsageHistoryWindow after = GetWindow(current, weekly);
            return before.ResetAtUtc.HasValue && after.ResetAtUtc.HasValue &&
                Math.Abs((before.ResetAtUtc.Value - after.ResetAtUtc.Value).TotalMinutes) <= 2;
        }

        private static bool IsFresh(UsageSnapshot snapshot, DateTime nowUtc)
        {
            if (!IsValidSnapshot(snapshot) || snapshot.IsPaused) return false;
            double age = (nowUtc - snapshot.LastUpdated.ToUniversalTime()).TotalMinutes;
            return age >= -1 && age <= 30;
        }

        private static bool IsValidSnapshot(UsageSnapshot snapshot)
        {
            return snapshot != null && !snapshot.IsStale && string.IsNullOrEmpty(snapshot.ErrorMessage) &&
                snapshot.LastUpdated != DateTime.MinValue && snapshot.HasPrimaryLimit &&
                IsValidLimit(snapshot.Weekly) && IsValidLimit(snapshot.FiveHour);
        }

        private static bool IsValidLimit(LimitWindow window)
        {
            return window == null || (IsValidPercent(window.UsedPercent) &&
                (!window.WindowMinutes.HasValue || window.WindowMinutes.Value > 0));
        }

        private static bool IsValidPercent(double value)
        {
            return !double.IsNaN(value) && !double.IsInfinity(value) && value >= 0 && value <= 100;
        }

        private static UsageHistoryWindow FromLimit(LimitWindow limit, DateTime timestamp)
        {
            if (limit == null) return null;
            return new UsageHistoryWindow
            {
                UsedPercent = limit.UsedPercent,
                WindowMinutes = limit.WindowMinutes,
                ResetAtUtc = limit.ResetAfterSeconds.HasValue
                    ? (DateTime?)timestamp.AddSeconds(limit.ResetAfterSeconds.Value) : null
            };
        }

        private void Prune(DateTime nowUtc)
        {
            DateTime earliest = nowUtc.AddDays(-14);
            samples.RemoveAll(delegate(UsageHistorySample sample) { return sample.TimestampUtc < earliest; });
            if (samples.Count > MaxSamples) samples.RemoveRange(0, samples.Count - MaxSamples);
        }

        private bool Load()
        {
            try
            {
                if (!File.Exists(path)) return false;
                string json;
                using (FileStream stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
                using (StreamReader reader = new StreamReader(stream, Encoding.UTF8))
                {
                    if (stream.Length > MaxFileBytes) return false;
                    json = reader.ReadToEnd();
                }
                JavaScriptSerializer serializer = new JavaScriptSerializer { MaxJsonLength = MaxFileBytes };
                List<UsageHistorySample> loaded = serializer.Deserialize<List<UsageHistorySample>>(json);
                if (loaded == null || loaded.Count > MaxSamples) return false;
                DateTime now = DateTime.UtcNow;
                DateTime previous = DateTime.MinValue;
                foreach (UsageHistorySample sample in loaded)
                {
                    if (sample == null || sample.TimestampUtc == DateTime.MinValue) continue;
                    sample.TimestampUtc = sample.TimestampUtc.ToUniversalTime();
                    if (sample.TimestampUtc <= previous || sample.TimestampUtc > now.AddMinutes(1) ||
                        !ValidateWindow(sample.Weekly) || !ValidateWindow(sample.FiveHour) ||
                        (sample.Weekly == null && sample.FiveHour == null)) continue;
                    samples.Add(sample);
                    previous = sample.TimestampUtc;
                }
                Prune(now);
                return true;
            }
            catch (Exception)
            {
                // History is optional; corrupt or inaccessible files never block usage refresh.
                samples.Clear();
                return false;
            }
        }

        private static bool ValidateWindow(UsageHistoryWindow window)
        {
            if (window == null) return true;
            if (!IsValidPercent(window.UsedPercent) || (window.WindowMinutes.HasValue && window.WindowMinutes <= 0)) return false;
            if (window.ResetAtUtc.HasValue) window.ResetAtUtc = window.ResetAtUtc.Value.ToUniversalTime();
            return true;
        }

        private bool Save()
        {
            string tempPath = null;
            try
            {
                string directory = Path.GetDirectoryName(Path.GetFullPath(path));
                Directory.CreateDirectory(directory);
                JavaScriptSerializer serializer = new JavaScriptSerializer { MaxJsonLength = MaxFileBytes };
                string json = serializer.Serialize(samples);
                if (Encoding.UTF8.GetByteCount(json) > MaxFileBytes) return false;
                tempPath = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
                File.WriteAllText(tempPath, json, new UTF8Encoding(false));
                if (File.Exists(path)) File.Replace(tempPath, path, null, true);
                else File.Move(tempPath, path);
                return true;
            }
            catch (Exception)
            {
                // Keep in-memory observations when the optional history file cannot be written.
                return false;
            }
            finally
            {
                try { if (tempPath != null && File.Exists(tempPath)) File.Delete(tempPath); }
                catch (Exception) { }
            }
        }
    }
}
