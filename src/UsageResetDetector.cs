using System;

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
}
