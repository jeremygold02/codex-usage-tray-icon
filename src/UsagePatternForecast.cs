using System;
using System.Collections.Generic;

namespace CodexUsageTray
{
    // Hourly patterns and recent pace candidates are compared on held-out readings.
    // Missing hours remain unknown, never assumed idle.
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
        public double InactiveHoursError;
        public bool IsValidated;
        public bool UsesDailyPattern;
        public bool UsesSessionPace;
        public bool UsesInactiveHours;
        public double? StartHourRate;

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
            double[] currentInactiveProfile = InactiveProfile(hours, samples, currentRate);
            double? currentStartRate = currentInactiveProfile != null && IsCurrentlyActive(samples)
                ? (double?)RecentActiveRate(samples, currentRate) : null;

            double paceError = 0;
            double patternError = 0;
            double dailyError = 0;
            double sessionError = 0;
            double inactiveError = 0;
            int count = 0;
            int inactiveCount = 0;
            int patternCount = 0;
            DateTime firstInactiveOrigin = DateTime.MinValue;
            DateTime lastInactiveOrigin = DateTime.MinValue;
            DateTime firstPatternOrigin = DateTime.MinValue;
            DateTime lastPatternOrigin = DateTime.MinValue;
            double validationWeight = 0;
            DateTime nextOrigin = latest.TimestampUtc.AddDays(-7);
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
                Dictionary<DateTime, Hour> trainingHours = BuildHours(training);
                double[] profile = Profile(trainingHours, origin.TimestampUtc, rate);
                double dailyRate = DailyRate(training, rate);
                double sessionRate;
                if (!UsageHistoryStore.TryGetSessionRate(snapshot, training, out sessionRate)) sessionRate = rate;
                double[] inactiveProfile = InactiveProfile(trainingHours, training, sessionRate);
                double? startRate = inactiveProfile != null && IsCurrentlyActive(training)
                    ? (double?)RecentActiveRate(training, sessionRate) : null;
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
                    double weight = Math.Pow(0.5, (latest.TimestampUtc - origin.TimestampUtc).TotalHours / 72);
                    double increase = Math.Max(0, actual.Weekly.UsedPercent - origin.Weekly.UsedPercent);
                    double allowance = 100 - origin.Weekly.UsedPercent;
                    // Normalize by the horizon so a 24-hour error cannot drown out
                    // the near-term forecasts used during an active session.
                    double baselineError = Math.Abs(Math.Min(allowance, rate * elapsed) - increase) / elapsed;
                    paceError += baselineError * weight;
                    dailyError += Math.Abs(Math.Min(allowance, dailyRate * elapsed) - increase) / elapsed * weight;
                    double sessionForecastError = Math.Abs(Math.Min(allowance, sessionRate * elapsed) - increase) / elapsed;
                    sessionError += sessionForecastError * weight;
                    inactiveError += (inactiveProfile == null ? sessionForecastError : Math.Abs(Math.Min(allowance,
                        Consumption(inactiveProfile, origin.TimestampUtc, actual.TimestampUtc, startRate)) - increase) / elapsed) * weight;
                    patternError += (profile == null ? baselineError : Math.Abs(Math.Min(allowance,
                        Consumption(profile, origin.TimestampUtc, actual.TimestampUtc)) - increase) / elapsed) * weight;
                    if (firstOrigin == DateTime.MinValue) firstOrigin = origin.TimestampUtc;
                    lastOrigin = origin.TimestampUtc;
                    count++;
                    validationWeight += weight;
                    if (inactiveProfile != null)
                    {
                        if (inactiveCount == 0) firstInactiveOrigin = origin.TimestampUtc;
                        lastInactiveOrigin = origin.TimestampUtc;
                        inactiveCount++;
                    }
                    if (profile != null)
                    {
                        if (patternCount == 0) firstPatternOrigin = origin.TimestampUtc;
                        lastPatternOrigin = origin.TimestampUtc;
                        patternCount++;
                    }
                }
            }
            // Require multiple evaluation days and a material improvement; a tie
            // keeps the simpler forecast. These are evidence gates, not usage rates.
            bool patternReady = currentProfile != null && patternCount >= 8 &&
                (lastPatternOrigin - firstPatternOrigin).TotalHours >= 24;
            bool useInactive = currentInactiveProfile != null && inactiveCount >= 8 &&
                (lastInactiveOrigin - firstInactiveOrigin).TotalHours >= 24 && inactiveError <= sessionError &&
                inactiveError < dailyError && (!patternReady || inactiveError <= patternError);
            bool usePattern = !useInactive && patternReady && patternError < Math.Min(dailyError, sessionError);
            bool useSession = useInactive || (!usePattern && sessionError < dailyError);
            if (useInactive) currentProfile = currentInactiveProfile;
            else if (!usePattern)
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
                PaceError = count == 0 ? 0 : paceError / validationWeight,
                PatternError = count == 0 ? 0 : patternError / validationWeight,
                DailyAverageError = count == 0 ? 0 : dailyError / validationWeight,
                SessionError = count == 0 ? 0 : sessionError / validationWeight,
                InactiveHoursError = count == 0 ? 0 : inactiveError / validationWeight,
                UsesDailyPattern = usePattern,
                UsesSessionPace = useSession,
                UsesInactiveHours = useInactive,
                StartHourRate = useInactive ? currentStartRate : null,
                IsValidated = count >= 8 && (lastOrigin - firstOrigin).TotalHours >= 24 &&
                    paceError > 0.001 && (useInactive ? inactiveError : usePattern ? patternError :
                        useSession ? sessionError : dailyError) < paceError * 0.9
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
                for (int day = 0; day < UsageHistoryStore.RetentionDays; day++)
                {
                    Hour hour;
                    if (!hours.TryGetValue(sameHour.AddDays(-day), out hour) || hour.Coverage < 0.9) continue;
                    double weight = Math.Pow(0.5, day / 7.0);
                    usage += hour.Used * weight;
                    coverage += hour.Coverage * weight;
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

        private static double[] InactiveProfile(Dictionary<DateTime, Hour> hours,
            List<UsageHistorySample> samples, double fallbackRate)
        {
            DateTime completed = FloorHour(samples[samples.Count - 1].TimestampUtc);
            // Weekday and weekend routines are learned separately so a quiet
            // weekend cannot turn a busy weekday morning into predicted sleep.
            bool[] quiet = new bool[48];
            List<DateTime> observedHours = new List<DateTime>(hours.Keys);
            observedHours.Sort(delegate(DateTime left, DateTime right) { return right.CompareTo(left); });
            for (int category = 0; category < 2; category++)
            for (int clockHour = 0; clockHour < 24; clockHour++)
            {
                int days = 0;
                bool inactive = true;
                HashSet<DateTime> observedDates = new HashSet<DateTime>();
                // Use the two most recent observed days, so a recent schedule
                // change supersedes an older weekend or previous work routine.
                foreach (DateTime time in observedHours)
                {
                    if (days == 2 || time < completed.AddDays(-UsageHistoryStore.RetentionDays)) break;
                    DateTime local = time.ToLocalTime();
                    Hour hour = hours[time];
                    if (time >= completed || DayCategory(time) != category || local.Hour != clockHour ||
                        hour.Coverage < 0.9 || !observedDates.Add(local.Date)) continue;
                    days++;
                    if (hour.Used > 0.001) inactive = false;
                }
                quiet[category * 24 + clockHour] = days == 2 && inactive;
            }
            bool[] sustained = new bool[48];
            int quietCount = 0;
            for (int category = 0; category < 2; category++)
            for (int clockHour = 0; clockHour < 24; clockHour++)
            {
                // Only repeated, contiguous quiet periods count; one flat hour
                // can simply be low consumption or percentage rounding.
                int offset = category * 24;
                bool inRun = (quiet[offset + (clockHour + 22) % 24] && quiet[offset + (clockHour + 23) % 24]) ||
                    (quiet[offset + (clockHour + 23) % 24] && quiet[offset + (clockHour + 1) % 24]) ||
                    (quiet[offset + (clockHour + 1) % 24] && quiet[offset + (clockHour + 2) % 24]);
                sustained[offset + clockHour] = quiet[offset + clockHour] && inRun;
                if (sustained[offset + clockHour]) quietCount++;
            }
            if (quietCount == 0 || quietCount == 48) return null;
            double activeRate = RecentActiveRate(samples, fallbackRate);
            double[] rates = new double[48];
            for (int clockHour = 0; clockHour < 48; clockHour++)
                rates[clockHour] = sustained[clockHour] ? 0 : activeRate;
            return rates;
        }

        private static double RecentActiveRate(List<UsageHistorySample> samples, double fallbackRate)
        {
            double currentRate;
            if (IsCurrentlyActive(samples) && UsageHistoryStore.TryGetSessionRate(
                Snapshot(samples[samples.Count - 1]), samples, out currentRate)) return currentRate;
            DateTime cutoff = samples[samples.Count - 1].TimestampUtc.AddDays(-7);
            for (int i = samples.Count - 1; i > 0; i--)
            {
                UsageHistorySample before = samples[i - 1];
                UsageHistorySample after = samples[i];
                if (after.TimestampUtc < cutoff || before.Weekly.WindowMinutes != after.Weekly.WindowMinutes) break;
                if (UsageHistoryStore.IsReset(before, after, true)) continue;
                double elapsed = (after.TimestampUtc - before.TimestampUtc).TotalHours;
                double increase = after.Weekly.UsedPercent - before.Weekly.UsedPercent;
                if (elapsed <= 0 || elapsed > 2 || increase <= 0.001) continue;
                List<UsageHistorySample> active = samples.GetRange(0, i + 1);
                double rate;
                return UsageHistoryStore.TryGetSessionRate(Snapshot(after), active, out rate)
                    ? rate : fallbackRate;
            }
            return fallbackRate;
        }

        private static bool IsCurrentlyActive(List<UsageHistorySample> samples)
        {
            DateTime cutoff = samples[samples.Count - 1].TimestampUtc.AddHours(-1);
            for (int i = samples.Count - 1; i > 0 && samples[i].TimestampUtc > cutoff; i--)
            {
                UsageHistorySample before = samples[i - 1];
                UsageHistorySample after = samples[i];
                if (UsageHistoryStore.IsReset(before, after, true) ||
                    before.Weekly.WindowMinutes != after.Weekly.WindowMinutes) break;
                if ((after.TimestampUtc - before.TimestampUtc).TotalHours <= 2 &&
                    after.Weekly.UsedPercent > before.Weekly.UsedPercent + 0.001) return true;
            }
            return false;
        }

        public static double Consumption(double[] rates, DateTime start, DateTime end, double? startHourRate = null)
        {
            double used = 0;
            DateTime firstHour = FloorHour(start);
            while (start < end)
            {
                DateTime next = FloorHour(start).AddHours(1);
                if (next > end) next = end;
                double rate = startHourRate.HasValue && FloorHour(start) == firstHour ? startHourRate.Value : RateAt(rates, start);
                used += rate * (next - start).TotalHours;
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
                UsesSessionPace = UsesSessionPace, UsesInactiveHours = UsesInactiveHours, UsesValidatedModel = true
            };
            projection.Points.Add(new UsageProjectionPoint { TimestampUtc = start, RemainingPercent = remaining });
            DateTime cursor = start;
            while (cursor < reset)
            {
                DateTime end = FloorHour(cursor).AddHours(1);
                if (end > reset) end = reset;
                double rate = StartHourRate.HasValue && FloorHour(cursor) == FloorHour(start)
                    ? StartHourRate.Value : RateAt(Rates, cursor);
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

        private static int DayCategory(DateTime time)
        {
            DayOfWeek day = time.ToLocalTime().DayOfWeek;
            return day == DayOfWeek.Saturday || day == DayOfWeek.Sunday ? 1 : 0;
        }

        private static double RateAt(double[] rates, DateTime time)
        {
            return rates.Length == 48 ? rates[time.ToLocalTime().Hour + DayCategory(time) * 24] : rates[time.Hour];
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
