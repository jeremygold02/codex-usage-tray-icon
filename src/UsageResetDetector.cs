using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using System.Web.Script.Serialization;

namespace CodexUsageTray
{
    [Flags]
    internal enum UsageResetKind
    {
        None = 0,
        Weekly = 1,
        FiveHour = 2
    }

    internal sealed class UsageResetDetector
    {
        private const int FullRemainingPercent = 100;
        private const int ImmediateRearmPercent = 98;
        private const int StableBelowFullObservations = 2;

        private readonly WindowState weeklyState = new WindowState();
        private readonly WindowState fiveHourState = new WindowState();

        public UsageResetKind Observe(UsageSnapshot snapshot)
        {
            if (snapshot == null)
            {
                return UsageResetKind.None;
            }

            UsageResetKind resets = UsageResetKind.None;
            if (ObserveWindow(weeklyState, snapshot.Weekly))
            {
                resets |= UsageResetKind.Weekly;
            }
            if (ObserveWindow(fiveHourState, snapshot.FiveHour))
            {
                resets |= UsageResetKind.FiveHour;
            }
            return resets;
        }

        private static bool ObserveWindow(WindowState state, LimitWindow window)
        {
            if (window == null)
            {
                return false;
            }

            int remaining = window.RemainingPercent;
            if (!state.HasObservation)
            {
                state.HasObservation = true;
                state.Armed = remaining < FullRemainingPercent;
                state.BelowFullObservations = remaining < FullRemainingPercent ? 1 : 0;
                return false;
            }

            if (remaining < FullRemainingPercent)
            {
                state.BelowFullObservations++;
                if (remaining <= ImmediateRearmPercent ||
                    state.BelowFullObservations >= StableBelowFullObservations)
                {
                    state.Armed = true;
                }
                return false;
            }

            bool resetDetected = state.Armed;
            state.Armed = false;
            state.BelowFullObservations = 0;
            return resetDetected;
        }

        private sealed class WindowState
        {
            public bool HasObservation;
            public bool Armed;
            public int BelowFullObservations;
        }
    }

    internal sealed class BankedResetDetector
    {
        private readonly HashSet<string> observedCreditIds =
            new HashSet<string>(StringComparer.Ordinal);
        private bool hasObservation;
        private bool previousItemizedListWasComplete;
        private int previousAvailableCount;

        public int CurrentAvailableCount { get; private set; }

        public BankedResetDetector()
            : this(null)
        {
        }

        public BankedResetDetector(BankedResetState state)
        {
            if (state == null || !state.HasObservation)
            {
                return;
            }

            hasObservation = true;
            previousAvailableCount = Math.Max(0, state.AvailableCount);
            CurrentAvailableCount = previousAvailableCount;
            previousItemizedListWasComplete = state.ItemizedListWasComplete;
            if (state.ObservedCreditIds != null)
            {
                for (int index = 0; index < state.ObservedCreditIds.Count; index++)
                {
                    string creditId = state.ObservedCreditIds[index];
                    if (!string.IsNullOrWhiteSpace(creditId))
                    {
                        observedCreditIds.Add(creditId);
                    }
                }
            }
        }

        public int Observe(UsageSnapshot snapshot)
        {
            if (!HasResetInformation(snapshot))
            {
                return 0;
            }

            int itemizedCount = snapshot.AvailableResets != null
                ? snapshot.AvailableResets.Count
                : 0;
            int reportedCount = snapshot.AvailableResetCount.HasValue
                ? Math.Max(0, snapshot.AvailableResetCount.Value)
                : 0;
            CurrentAvailableCount = Math.Max(itemizedCount, reportedCount);
            bool itemizedListIsComplete = itemizedCount >= CurrentAvailableCount;
            int newItemizedCount = CountNewCreditIds(snapshot.AvailableResets);

            if (!hasObservation)
            {
                hasObservation = true;
                previousAvailableCount = CurrentAvailableCount;
                previousItemizedListWasComplete = itemizedListIsComplete;
                return 0;
            }

            int countIncrease = Math.Max(0, CurrentAvailableCount - previousAvailableCount);
            int addedCount = previousItemizedListWasComplete && itemizedListIsComplete
                ? Math.Max(countIncrease, newItemizedCount)
                : countIncrease;
            previousAvailableCount = CurrentAvailableCount;
            previousItemizedListWasComplete = itemizedListIsComplete;
            return addedCount;
        }

        public BankedResetState CreateState()
        {
            BankedResetState state = new BankedResetState();
            state.HasObservation = hasObservation;
            state.AvailableCount = previousAvailableCount;
            state.ItemizedListWasComplete = previousItemizedListWasComplete;
            state.ObservedCreditIds = new List<string>(observedCreditIds);
            state.ObservedCreditIds.Sort(StringComparer.Ordinal);
            return state;
        }

        private int CountNewCreditIds(IList<RateLimitResetCredit> credits)
        {
            if (credits == null)
            {
                return 0;
            }

            int newCount = 0;
            for (int index = 0; index < credits.Count; index++)
            {
                RateLimitResetCredit credit = credits[index];
                if (credit != null &&
                    !string.IsNullOrWhiteSpace(credit.Id) &&
                    observedCreditIds.Add(credit.Id))
                {
                    newCount++;
                }
            }
            return newCount;
        }

        private static bool HasResetInformation(UsageSnapshot snapshot)
        {
            return snapshot != null &&
                (snapshot.AvailableResetCount.HasValue ||
                    (snapshot.AvailableResets != null && snapshot.AvailableResets.Count > 0));
        }
    }

    internal sealed class BankedResetState
    {
        public int Version { get; set; }
        public bool HasObservation { get; set; }
        public int AvailableCount { get; set; }
        public bool ItemizedListWasComplete { get; set; }
        public List<string> ObservedCreditIds { get; set; }

        public BankedResetState()
        {
            Version = BankedResetStateStore.CurrentVersion;
            ObservedCreditIds = new List<string>();
        }
    }

    internal sealed class BankedResetStateStore
    {
        internal const int CurrentVersion = 1;
        private readonly string path;

        public BankedResetStateStore()
            : this(GetDefaultPath())
        {
        }

        internal BankedResetStateStore(string path)
        {
            this.path = path;
        }

        public BankedResetState Load()
        {
            try
            {
                if (!File.Exists(path))
                {
                    return new BankedResetState();
                }

                BankedResetState state = new JavaScriptSerializer()
                    .Deserialize<BankedResetState>(File.ReadAllText(path));
                if (state == null || state.Version != CurrentVersion || state.AvailableCount < 0)
                {
                    return new BankedResetState();
                }
                if (state.ObservedCreditIds == null)
                {
                    state.ObservedCreditIds = new List<string>();
                }
                return state;
            }
            catch
            {
                return new BankedResetState();
            }
        }

        public void Save(BankedResetState state)
        {
            if (state == null)
            {
                return;
            }

            string tempPath = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            try
            {
                string directory = Path.GetDirectoryName(path);
                if (string.IsNullOrEmpty(directory))
                {
                    return;
                }
                Directory.CreateDirectory(directory);
                string json = new JavaScriptSerializer().Serialize(state);
                File.WriteAllText(tempPath, json, new UTF8Encoding(false));
                if (File.Exists(path))
                {
                    File.Replace(tempPath, path, null, true);
                }
                else
                {
                    File.Move(tempPath, path);
                }
            }
            catch
            {
            }
            finally
            {
                try
                {
                    if (File.Exists(tempPath))
                    {
                        File.Delete(tempPath);
                    }
                }
                catch
                {
                }
            }
        }

        private static string GetDefaultPath()
        {
            return Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                "CodexUsageTray",
                "banked-reset-state.json");
        }
    }

    internal sealed class UsageResetNotificationSuppression
    {
        private DateTime suppressThroughUtc = DateTime.MinValue;

        public void SuppressFor(DateTime nowUtc, TimeSpan duration)
        {
            if (duration <= TimeSpan.Zero)
            {
                suppressThroughUtc = DateTime.MinValue;
                return;
            }

            DateTime candidate = NormalizeUtc(nowUtc).Add(duration);
            if (candidate > suppressThroughUtc)
            {
                suppressThroughUtc = candidate;
            }
        }

        public UsageResetKind Filter(UsageResetKind detectedResets, DateTime nowUtc)
        {
            if (suppressThroughUtc == DateTime.MinValue)
            {
                return detectedResets;
            }

            if (NormalizeUtc(nowUtc) > suppressThroughUtc)
            {
                suppressThroughUtc = DateTime.MinValue;
                return detectedResets;
            }

            return UsageResetKind.None;
        }

        private static DateTime NormalizeUtc(DateTime value)
        {
            if (value.Kind == DateTimeKind.Utc)
            {
                return value;
            }
            if (value.Kind == DateTimeKind.Local)
            {
                return value.ToUniversalTime();
            }

            return DateTime.SpecifyKind(value, DateTimeKind.Utc);
        }
    }
}
