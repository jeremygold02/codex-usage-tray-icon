using System;
using System.Collections.Generic;

namespace CodexUsageTray
{
    // A daily seasonal mean, selected by rolling-origin validation against the
    // existing pace forecast. Missing hours remain unknown, never assumed idle.
    internal sealed class UsagePatternForecast
    {
        private sealed class Hour
        {
            public double Used;
            public double Coverage;
        }

        public double[] Rates;
        public int ValidationCount;
        public double PaceError;
        public double PatternError;
        public double DailyAverageError;
        public double SessionError;
        public bool IsValidated;
        public bool UsesDailyPattern;
        public bool UsesSessionPace;

        public static UsagePatternForecast TryCreate(List<UsageHistorySample> samples, double currentRate)
        {
            UsagePatternForecast candidate = Evaluate(samples, currentRate);
            return candidate != null && candidate.IsValidated ? candidate : null;
        }

        internal static UsagePatternForecast Evaluate(List<UsageHistorySample> samples, double currentRate)
        {
            if (samples.Count < 3) return null;
            UsageHistorySample latest = samples[samples.Count - 1];
            Dictionary<DateTime, Hour> hours = BuildHours(samples);
            double[] currentProfile = Profile(hours, latest.TimestampUtc, currentRate);

            double paceError = 0;
            double patternError = 0;
            double dailyError = 0;
            double sessionError = 0;
            int count = 0;
            DateTime nextOrigin = latest.TimestampUtc.AddDays(-3);
            DateTime firstOrigin = DateTime.MinValue;
            DateTime lastOrigin = DateTime.MinValue;
            for (int i = 0; i < samples.Count - 1; i++)
            {
                UsageHistorySample origin = samples[i];
                if (origin.TimestampUtc < nextOrigin || origin.Weekly.UsedPercent >= 100 ||
                    origin.Weekly.WindowMinutes != latest.Weekly.WindowMinutes) continue;
                List<UsageHistorySample> training = samples.GetRange(0, i + 1);
                UsageSnapshot snapshot = Snapshot(origin);
                double rate;
                if (!UsageHistoryStore.TryGetRecentRate(true, snapshot, training, out rate) &&
                    !UsageHistoryStore.TryGetCycleAverageRate(true, snapshot, training, out rate)) continue;
                // Profile uses only completed hours before this origin; the held-out
                // readings must not affect either candidate's training data.
                double[] profile = Profile(BuildHours(training), origin.TimestampUtc, rate);
                double dailyRate = DailyRate(training, rate);
                double sessionRate;
                if (!UsageHistoryStore.TryGetSessionRate(snapshot, training, out sessionRate)) sessionRate = rate;
                nextOrigin = origin.TimestampUtc.AddHours(3);
                int target = i + 1;
                foreach (int horizon in new[] { 1, 6, 24 })
                {
                    DateTime deadline = origin.TimestampUtc.AddHours(horizon);
                    bool crossedReset = false;
                    while (target < samples.Count)
                    {
                        if (samples[target].Weekly.WindowMinutes != origin.Weekly.WindowMinutes ||
                            UsageHistoryStore.IsReset(samples[target - 1], samples[target], true))
                        {
                            crossedReset = true;
                            break;
                        }
                        if (samples[target].TimestampUtc >= deadline) break;
                        target++;
                    }
                    if (crossedReset || target >= samples.Count) break;
                    UsageHistorySample actual = samples[target];
                    if ((actual.TimestampUtc - deadline).TotalHours > (horizon == 1 ? 0.25 : 1) ||
                        actual.Weekly.UsedPercent >= 100) continue;
                    double elapsed = (actual.TimestampUtc - origin.TimestampUtc).TotalHours;
                    double increase = Math.Max(0, actual.Weekly.UsedPercent - origin.Weekly.UsedPercent);
                    double allowance = 100 - origin.Weekly.UsedPercent;
                    // Normalize by the horizon so a 24-hour error cannot drown out
                    // the near-term forecasts used during an active session.
                    double baselineError = Math.Abs(Math.Min(allowance, rate * elapsed) - increase) / elapsed;
                    paceError += baselineError;
                    dailyError += Math.Abs(Math.Min(allowance, dailyRate * elapsed) - increase) / elapsed;
                    sessionError += Math.Abs(Math.Min(allowance, sessionRate * elapsed) - increase) / elapsed;
                    patternError += profile == null ? baselineError : Math.Abs(Math.Min(allowance,
                        Consumption(profile, origin.TimestampUtc, actual.TimestampUtc)) - increase) / elapsed;
                    if (firstOrigin == DateTime.MinValue) firstOrigin = origin.TimestampUtc;
                    lastOrigin = origin.TimestampUtc;
                    count++;
                }
            }
            // Require multiple evaluation days and a material improvement; a tie
            // keeps the simpler forecast. These are evidence gates, not usage rates.
            bool usePattern = currentProfile != null && patternError < Math.Min(dailyError, sessionError);
            bool useSession = !usePattern && sessionError < dailyError;
            if (!usePattern)
            {
                double rate = DailyRate(samples, currentRate);
                if (useSession && !UsageHistoryStore.TryGetSessionRate(Snapshot(latest), samples, out rate))
                    rate = currentRate;
                currentProfile = new double[24];
                for (int hour = 0; hour < 24; hour++) currentProfile[hour] = rate;
            }
            return new UsagePatternForecast
            {
                Rates = currentProfile, ValidationCount = count,
                PaceError = count == 0 ? 0 : paceError / count,
                PatternError = count == 0 ? 0 : patternError / count,
                DailyAverageError = count == 0 ? 0 : dailyError / count,
                SessionError = count == 0 ? 0 : sessionError / count,
                UsesDailyPattern = usePattern,
                UsesSessionPace = useSession,
                IsValidated = count >= 8 && (lastOrigin - firstOrigin).TotalHours >= 24 &&
                    paceError > 0.001 && (usePattern ? patternError : useSession ? sessionError : dailyError) < paceError * 0.9
            };
        }

        private static double DailyRate(List<UsageHistorySample> samples, double fallback)
        {
            int end = samples.Count - 1;
            int start = end;
            DateTime cutoff = samples[end].TimestampUtc.AddHours(-24);
            while (start > 0 && samples[start - 1].TimestampUtc >= cutoff)
            {
                UsageHistorySample before = samples[start - 1];
                UsageHistorySample after = samples[start];
                if (before.Weekly.WindowMinutes != after.Weekly.WindowMinutes ||
                    UsageHistoryStore.IsReset(before, after, true) ||
                    ((after.TimestampUtc - before.TimestampUtc).TotalHours > 2 && !MatchingReset(before, after))) break;
                start--;
            }
            double hours = (samples[end].TimestampUtc - samples[start].TimestampUtc).TotalHours;
            return end - start >= 2 && hours >= 6
                ? Math.Max(0, samples[end].Weekly.UsedPercent - samples[start].Weekly.UsedPercent) / hours : fallback;
        }

        private static Dictionary<DateTime, Hour> BuildHours(List<UsageHistorySample> samples)
        {
            Dictionary<DateTime, Hour> hours = new Dictionary<DateTime, Hour>();
            for (int i = 1; i < samples.Count; i++)
            {
                UsageHistorySample before = samples[i - 1];
                UsageHistorySample after = samples[i];
                if (before.Weekly.WindowMinutes != samples[samples.Count - 1].Weekly.WindowMinutes ||
                    before.Weekly.WindowMinutes != after.Weekly.WindowMinutes ||
                    UsageHistoryStore.IsReset(before, after, true)) continue;
                double elapsed = (after.TimestampUtc - before.TimestampUtc).TotalHours;
                double increase = Math.Max(0, after.Weekly.UsedPercent - before.Weekly.UsedPercent);
                if (elapsed <= 0 || before.Weekly.UsedPercent >= 100 || after.Weekly.UsedPercent >= 100) continue;
                // A long positive gap cannot reveal which hours were busy. A flat
                // gap is evidence of inactivity only with a matching reset deadline.
                if (elapsed > 2 && (increase > 0 || !MatchingReset(before, after))) continue;
                double rate = increase / elapsed;
                DateTime cursor = before.TimestampUtc;
                while (cursor < after.TimestampUtc)
                {
                    DateTime hourStart = FloorHour(cursor);
                    DateTime end = hourStart.AddHours(1);
                    if (end > after.TimestampUtc) end = after.TimestampUtc;
                    double duration = (end - cursor).TotalHours;
                    Hour hour;
                    if (!hours.TryGetValue(hourStart, out hour))
                    {
                        hour = new Hour();
                        hours.Add(hourStart, hour);
                    }
                    hour.Used += rate * duration;
                    hour.Coverage += duration;
                    cursor = end;
                }
            }
            return hours;
        }

        private static double[] Profile(Dictionary<DateTime, Hour> hours, DateTime origin, double fallbackRate)
        {
            double[] rates = new double[24];
            int supported = 0;
            DateTime completed = FloorHour(origin);
            for (int clockHour = 0; clockHour < 24; clockHour++)
            {
                double usage = 0;
                double coverage = 0;
                int days = 0;
                DateTime sameHour = completed.Date.AddHours(clockHour);
                if (sameHour >= completed) sameHour = sameHour.AddDays(-1);
                for (int day = 0; day < 7; day++)
                {
                    Hour hour;
                    if (!hours.TryGetValue(sameHour.AddDays(-day), out hour) || hour.Coverage < 0.9) continue;
                    usage += hour.Used;
                    coverage += hour.Coverage;
                    days++;
                }
                if (days >= 3)
                {
                    rates[clockHour] = usage / coverage;
                    supported++;
                }
                else rates[clockHour] = fallbackRate;
            }
            if (supported < 8) return null;
            // Separate the daily shape from its current level. Match the previous
            // day's observed consumption against those same hours of the profile;
            // this learns a scale factor instead of projecting old activity levels.
            double recentUsage = 0;
            double expectedUsage = 0;
            double recentCoverage = 0;
            for (DateTime time = completed.AddHours(-24); time < completed; time = time.AddHours(1))
            {
                Hour hour;
                if (!hours.TryGetValue(time, out hour) || hour.Coverage < 0.9) continue;
                recentUsage += hour.Used;
                expectedUsage += rates[time.Hour] * hour.Coverage;
                recentCoverage += hour.Coverage;
            }
            if (recentCoverage >= 18 && expectedUsage > 0.001)
            {
                double scale = recentUsage / expectedUsage;
                for (int hour = 0; hour < 24; hour++) rates[hour] *= scale;
            }
            return rates;
        }

        public static double Consumption(double[] rates, DateTime start, DateTime end)
        {
            double used = 0;
            while (start < end)
            {
                DateTime next = FloorHour(start).AddHours(1);
                if (next > end) next = end;
                used += rates[start.Hour] * (next - start).TotalHours;
                start = next;
            }
            return used;
        }

        public UsageProjection Project(DateTime start, DateTime reset, double remaining)
        {
            UsageProjection projection = new UsageProjection
            {
                StartUtc = start, StartRemainingPercent = remaining,
                Points = new List<UsageProjectionPoint>(), UsesDailyPattern = UsesDailyPattern,
                UsesSessionPace = UsesSessionPace, UsesValidatedModel = true
            };
            projection.Points.Add(new UsageProjectionPoint { TimestampUtc = start, RemainingPercent = remaining });
            DateTime cursor = start;
            while (cursor < reset)
            {
                DateTime end = FloorHour(cursor).AddHours(1);
                if (end > reset) end = reset;
                double rate = Rates[cursor.Hour];
                double increase = rate * (end - cursor).TotalHours;
                if (rate > 0 && increase >= remaining)
                {
                    end = cursor.AddHours(remaining / rate);
                    remaining = 0;
                }
                else remaining = Math.Max(0, remaining - increase);
                projection.Points.Add(new UsageProjectionPoint { TimestampUtc = end, RemainingPercent = remaining });
                cursor = end;
                if (remaining <= 0) break;
            }
            projection.EndUtc = cursor;
            projection.EndRemainingPercent = remaining;
            projection.EndsAtReset = cursor >= reset;
            return projection;
        }

        private static DateTime FloorHour(DateTime value)
        {
            return new DateTime(value.Year, value.Month, value.Day, value.Hour, 0, 0, DateTimeKind.Utc);
        }

        private static bool MatchingReset(UsageHistorySample before, UsageHistorySample after)
        {
            return before.Weekly.ResetAtUtc.HasValue && after.Weekly.ResetAtUtc.HasValue &&
                Math.Abs((before.Weekly.ResetAtUtc.Value - after.Weekly.ResetAtUtc.Value).TotalMinutes) <= 2;
        }

        private static UsageSnapshot Snapshot(UsageHistorySample sample)
        {
            return new UsageSnapshot
            {
                LastUpdated = sample.TimestampUtc,
                Weekly = new LimitWindow
                {
                    UsedPercent = sample.Weekly.UsedPercent, WindowMinutes = sample.Weekly.WindowMinutes,
                    ResetAfterSeconds = sample.Weekly.ResetAtUtc.HasValue
                        ? (int?)(int)(sample.Weekly.ResetAtUtc.Value - sample.TimestampUtc).TotalSeconds : null
                }
            };
        }
    }
}
