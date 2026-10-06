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
        public bool CycleAverageUnavailable;
        public bool IsImported;

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

    internal sealed class UsageProjectionPoint
    {
        public DateTime TimestampUtc;
        public double RemainingPercent;
    }

    internal sealed class UsageProjection
    {
        public DateTime StartUtc;
        public DateTime EndUtc;
        public double StartRemainingPercent;
        public double EndRemainingPercent;
        public bool EndsAtReset;
        public bool UsesDailyPattern;
        public bool UsesValidatedModel;
        public bool UsesSessionPace;
        public bool UsesInactiveHours;
        public List<UsageProjectionPoint> Points;
    }

    internal sealed class UsageHistoryStore
    {
        private const int MaxSamples = 45000;
        internal const int RetentionDays = 28;
        private const int MaxFileBytes = 16 * 1024 * 1024;
        private const string ImportVersion = "2";
        private readonly string path;
        private readonly List<UsageHistorySample> samples = new List<UsageHistorySample>();
        private UsagePatternForecast patternForecast;
        private DateTime patternTimestamp;
        private int patternSampleCount = -1;
        private double patternRate;
        public bool ImportCompleted { get; private set; }

        public UsageHistoryStore(string path)
        {
            this.path = path;
            bool loaded = Load();
            if (loaded)
            {
                try { ImportCompleted = File.ReadAllText(path + ".imported").Trim() == ImportVersion; }
                catch (Exception) { }
            }
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
            if (timestamp < now.AddDays(-RetentionDays) || timestamp > now.AddMinutes(1) ||
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

        public int Import(IEnumerable<UsageSnapshot> snapshots, bool incremental = false, bool completed = true)
        {
            if (snapshots == null || (ImportCompleted && !incremental)) return 0;
            DateTime now = DateTime.UtcNow;
            SortedDictionary<DateTime, UsageHistorySample> merged = new SortedDictionary<DateTime, UsageHistorySample>();
            foreach (UsageHistorySample sample in samples) merged[sample.TimestampUtc] = sample;
            int added = 0;
            foreach (UsageSnapshot snapshot in snapshots)
            {
                if (!IsValidSnapshot(snapshot) || snapshot.IsPaused || snapshot.IsRefreshing) continue;
                DateTime timestamp = snapshot.LastUpdated.ToUniversalTime();
                if (timestamp < now.AddDays(-RetentionDays) || timestamp > now) continue;
                UsageHistorySample existing;
                if (!merged.TryGetValue(timestamp, out existing))
                {
                    merged[timestamp] = new UsageHistorySample
                    {
                        TimestampUtc = timestamp,
                        Weekly = FromLimit(snapshot.Weekly, timestamp, true),
                        FiveHour = FromLimit(snapshot.FiveHour, timestamp, true)
                    };
                    added++;
                }
                else
                {
                    // Live observations take precedence; imports can fill an absent window.
                    if (existing.Weekly == null && snapshot.Weekly != null)
                    {
                        existing.Weekly = FromLimit(snapshot.Weekly, timestamp, true);
                        added++;
                    }
                    if (existing.FiveHour == null && snapshot.FiveHour != null)
                    {
                        existing.FiveHour = FromLimit(snapshot.FiveHour, timestamp, true);
                        added++;
                    }
                }
            }
            if (added == 0 && ImportCompleted) return 0;
            samples.Clear();
            samples.AddRange(merged.Values);
            FilterImportedWindows(true);
            FilterImportedWindows(false);
            Prune(now);
            bool saved = Save();
            if (completed) MarkImportCompleted(saved);
            return added;
        }

        private void MarkImportCompleted(bool persist)
        {
            ImportCompleted = true;
            if (!persist) return;
            try
            {
                Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path)));
                File.WriteAllText(path + ".imported", ImportVersion);
            }
            catch (Exception) { }
        }

        private void FilterImportedWindows(bool weekly)
        {
            List<UsageHistorySample> live = samples.FindAll(delegate(UsageHistorySample sample)
            {
                UsageHistoryWindow window = GetWindow(sample, weekly);
                return window != null && !window.IsImported;
            });
            int nextLive = 0;
            UsageHistorySample previous = null;
            foreach (UsageHistorySample sample in samples)
            {
                UsageHistoryWindow window = GetWindow(sample, weekly);
                if (window == null) continue;
                while (nextLive < live.Count && live[nextLive].TimestampUtc < sample.TimestampUtc) nextLive++;
                bool stale = false;
                if (window.IsImported)
                {
                    // Logs can re-emit cached quota from an older API response. Such
                    // a decrease must not create a reset or contradict a live reading.
                    if (previous != null && HasMatchingReset(previous, sample, weekly) &&
                        window.UsedPercent < GetWindow(previous, weekly).UsedPercent - 0.001) stale = true;
                    if (nextLive < live.Count && HasMatchingReset(sample, live[nextLive], weekly) &&
                        window.UsedPercent > GetWindow(live[nextLive], weekly).UsedPercent + 0.001) stale = true;
                    if (previous != null && GetWindow(previous, weekly).ResetAtUtc.HasValue && window.ResetAtUtc.HasValue &&
                        window.ResetAtUtc.Value < GetWindow(previous, weekly).ResetAtUtc.Value.AddMinutes(-2)) stale = true;
                }
                if (stale)
                {
                    if (weekly) sample.Weekly = null;
                    else sample.FiveHour = null;
                }
                else previous = sample;
            }
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
            if (weekly)
            {
                UsageProjection pattern = GetProjection(true, snapshot, nowUtc);
                if (pattern != null && pattern.UsesValidatedModel)
                    return pattern.EndsAtReset ? "Will last until reset" : "\u2248" +
                        FormatDuration((pattern.EndUtc - pattern.StartUtc).TotalHours) +
                        (pattern.UsesInactiveHours ? " left with usual inactive hours" :
                            pattern.UsesDailyPattern ? " left at usual daily pattern" :
                            pattern.UsesSessionPace ? " left at recent session pace" : " left at recent daily pace");
            }
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
            if (!IsFresh(snapshot, nowUtc)) return null;
            LimitWindow limit = weekly ? snapshot.Weekly : snapshot.FiveHour;
            if (limit == null || limit.UsedPercent >= 100 || !limit.ResetAfterSeconds.HasValue) return null;

            DateTime startUtc = snapshot.LastUpdated.ToUniversalTime();
            DateTime resetUtc = startUtc.AddSeconds(limit.ResetAfterSeconds.Value);
            if (resetUtc <= nowUtc) return null;

            List<UsageHistorySample> periodSamples = samples.FindAll(
                delegate(UsageHistorySample sample) { return GetWindow(sample, weekly) != null; });
            double ratePerHour;
            bool hasRate = TryGetRecentRate(weekly, snapshot, periodSamples, out ratePerHour) ||
                TryGetCycleAverageRate(weekly, snapshot, periodSamples, out ratePerHour);

            double remaining = 100 - limit.UsedPercent;
            if (weekly && periodSamples.Count > 0)
            {
                UsageHistorySample latest = periodSamples[periodSamples.Count - 1];
                if (Math.Abs((latest.TimestampUtc - startUtc).TotalSeconds) <= 1 &&
                    Math.Abs(latest.Weekly.UsedPercent - limit.UsedPercent) < 0.001)
                {
                    UsagePatternForecast pattern = GetPatternForecast(periodSamples, ratePerHour);
                    if (pattern != null) return pattern.Project(startUtc, resetUtc, remaining);
                }
            }
            if (!hasRate) return null;
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

        private UsagePatternForecast GetPatternForecast(List<UsageHistorySample> periodSamples, double ratePerHour)
        {
            DateTime timestamp = periodSamples[periodSamples.Count - 1].TimestampUtc;
            if (patternSampleCount != periodSamples.Count || patternTimestamp != timestamp || patternRate != ratePerHour)
            {
                patternForecast = UsagePatternForecast.TryCreate(periodSamples, ratePerHour);
                patternTimestamp = timestamp;
                patternSampleCount = periodSamples.Count;
                patternRate = ratePerHour;
            }
            return patternForecast;
        }

        internal static bool TryGetRecentRate(bool weekly, UsageSnapshot snapshot,
            List<UsageHistorySample> periodSamples, out double ratePerHour)
        {
            return TryGetRecentRate(weekly, snapshot, periodSamples, weekly ? 48 : 1,
                weekly ? 6 : 0.25, weekly ? 6 : 0.25, weekly, out ratePerHour);
        }

        internal static bool TryGetSessionRate(UsageSnapshot snapshot,
            List<UsageHistorySample> periodSamples, out double ratePerHour)
        {
            return TryGetRecentRate(true, snapshot, periodSamples, 6, 1, 0.5, false, out ratePerHour);
        }

        private static bool TryGetRecentRate(bool weekly, UsageSnapshot snapshot,
            List<UsageHistorySample> periodSamples, double lookbackHours, double halfLifeHours,
            double minimumHours, bool bridgeGaps, out double ratePerHour)
        {
            ratePerHour = 0;
            if (periodSamples.Count < 3) return false;
            LimitWindow limit = weekly ? snapshot.Weekly : snapshot.FiveHour;
            int end = periodSamples.Count - 1;
            UsageHistorySample last = periodSamples[end];
            UsageHistoryWindow lastWindow = GetWindow(last, weekly);
            if (lastWindow == null || Math.Abs((last.TimestampUtc - snapshot.LastUpdated.ToUniversalTime()).TotalSeconds) > 1 ||
                Math.Abs(lastWindow.UsedPercent - limit.UsedPercent) > 0.001) return false;

            DateTime cutoff = last.TimestampUtc.AddHours(-lookbackHours);
            int start = end;
            while (start > 0 && periodSamples[start - 1].TimestampUtc >= cutoff)
            {
                UsageHistorySample before = periodSamples[start - 1];
                UsageHistorySample after = periodSamples[start];
                if (!SameCycle(before, after, weekly)) break;
                // Overnight observations still give a valid weekly wall-clock average
                // when a known reset deadline proves they belong to the same cycle.
                if (!IsContinuous(before, after, weekly) &&
                    (!bridgeGaps || !HasMatchingReset(before, after, weekly))) break;
                start--;
            }

            double hours = (last.TimestampUtc - periodSamples[start].TimestampUtc).TotalHours;
            if (end - start < 2 || hours < minimumHours) return false;
            // Weight elapsed time, not sample count: frequent polling must not amplify
            // a burst. Integrating the decay over each interval also keeps a steady
            // rate unchanged when the same interval is split into more observations.
            double decayPerHour = Math.Log(2) / halfLifeHours;
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

        internal static bool TryGetCycleAverageRate(bool weekly, UsageSnapshot snapshot,
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
            if (periodSamples.Count > 0)
            {
                UsageHistoryWindow last = GetWindow(periodSamples[periodSamples.Count - 1], weekly);
                if (last.CycleAverageUnavailable && last.ResetAtUtc.HasValue &&
                    Math.Abs((last.ResetAtUtc.Value - reset).TotalMinutes) <= 2) return false;
            }
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
            double minutes = Math.Round(hours * 60, MidpointRounding.AwayFromZero);
            double wholeHours = Math.Floor(minutes / 60);
            double remainingMinutes = minutes % 60;
            if (hours >= 24)
            {
                double days = Math.Round(hours / 24, 1, MidpointRounding.AwayFromZero);
                return days.ToString("0.#", CultureInfo.InvariantCulture) + " day" + (days == 1 ? "" : "s") +
                    " (" + wholeHours.ToString("0", CultureInfo.InvariantCulture) + "h " +
                    remainingMinutes.ToString("0", CultureInfo.InvariantCulture) + "m)";
            }
            string hourText = wholeHours.ToString("0", CultureInfo.InvariantCulture) + " hour" + (wholeHours == 1 ? "" : "s");
            string minuteText = remainingMinutes.ToString("0", CultureInfo.InvariantCulture) + " minute" + (remainingMinutes == 1 ? "" : "s");
            if (wholeHours == 0) return minuteText;
            return remainingMinutes == 0 ? hourText : hourText + " " + minuteText;
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

        private static UsageHistoryWindow FromLimit(LimitWindow limit, DateTime timestamp, bool imported = false)
        {
            if (limit == null) return null;
            return new UsageHistoryWindow
            {
                UsedPercent = limit.UsedPercent,
                IsImported = imported,
                WindowMinutes = limit.WindowMinutes,
                ResetAtUtc = limit.ResetAfterSeconds.HasValue
                    ? (DateTime?)timestamp.AddSeconds(limit.ResetAfterSeconds.Value) : null
            };
        }

        private void Prune(DateTime nowUtc)
        {
            DateTime earliest = nowUtc.AddDays(-RetentionDays);
            samples.RemoveAll(delegate(UsageHistorySample sample) { return sample.TimestampUtc < earliest; });
            samples.RemoveAll(delegate(UsageHistorySample sample) { return sample.Weekly == null && sample.FiveHour == null; });
            CoalesceSamples();
            if (samples.Count > MaxSamples) samples.RemoveRange(0, samples.Count - MaxSamples);
            MarkCycleAdjustments(true);
            MarkCycleAdjustments(false);
            samples.RemoveAll(delegate(UsageHistorySample sample) { return sample.Weekly == null && sample.FiveHour == null; });
            patternSampleCount = -1;
        }

        private void CoalesceSamples()
        {
            int count = 0;
            for (int i = 0; i < samples.Count; i++)
            {
                UsageHistorySample sample = samples[i];
                if (count >= 3 && (sample.TimestampUtc - samples[count - 2].TimestampUtc).TotalHours <= 1 &&
                    SameReading(samples[count - 3], samples[count - 2]) &&
                    SameReading(samples[count - 2], samples[count - 1]) && SameReading(samples[count - 1], sample))
                    samples[count - 1] = sample;
                else samples[count++] = sample;
            }
            if (count < samples.Count) samples.RemoveRange(count, samples.Count - count);
        }

        private static bool SameReading(UsageHistorySample left, UsageHistorySample right)
        {
            return SameWindow(left.Weekly, right.Weekly) && SameWindow(left.FiveHour, right.FiveHour);
        }

        private static bool SameWindow(UsageHistoryWindow left, UsageHistoryWindow right)
        {
            if (left == null || right == null) return left == right;
            return left.UsedPercent == right.UsedPercent && left.WindowMinutes == right.WindowMinutes &&
                left.IsImported == right.IsImported && left.CycleAverageUnavailable == right.CycleAverageUnavailable &&
                left.ResetAtUtc.HasValue == right.ResetAtUtc.HasValue && (!left.ResetAtUtc.HasValue ||
                Math.Abs((left.ResetAtUtc.Value - right.ResetAtUtc.Value).TotalSeconds) <= 2);
        }

        private void MarkCycleAdjustments(bool weekly)
        {
            UsageHistorySample previous = null;
            bool invalidAverage = false;
            foreach (UsageHistorySample sample in samples)
            {
                UsageHistoryWindow window = GetWindow(sample, weekly);
                if (window == null) continue;
                if (previous != null && IsReset(previous, sample, weekly))
                {
                    invalidAverage = HasMatchingReset(previous, sample, weekly) &&
                        window.UsedPercent < GetWindow(previous, weekly).UsedPercent - 0.001;
                }
                invalidAverage = invalidAverage || window.CycleAverageUnavailable;
                window.CycleAverageUnavailable = invalidAverage;
                previous = sample;
            }
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
                int beforePruning = samples.Count;
                Prune(now);
                if (samples.Count != beforePruning) Save();
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
                for (int attempt = 0; ; attempt++)
                {
                    try
                    {
                        if (File.Exists(path)) File.Replace(tempPath, path, null, true);
                        else File.Move(tempPath, path);
                        break;
                    }
                    catch (IOException)
                    {
                        // Windows can briefly lock the replaced file. Keep atomic
                        // writes and allow two short retries before retaining memory only.
                        if (attempt >= 2) throw;
                        System.Threading.Thread.Sleep(25);
                    }
                }
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
