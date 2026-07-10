using System;

namespace CodexUsageTray
{
    internal sealed class LimitWindow
    {
        public double UsedPercent;
        public int? WindowMinutes;
        public int? ResetAfterSeconds;

        public int RemainingPercent
        {
            get
            {
                double remaining = 100.0 - UsedPercent;
                if (remaining < 0)
                {
                    remaining = 0;
                }
                if (remaining > 100)
                {
                    remaining = 100;
                }
                return (int)Math.Round(remaining);
            }
        }

        public LimitWindow Clone()
        {
            return (LimitWindow)MemberwiseClone();
        }
    }
}
