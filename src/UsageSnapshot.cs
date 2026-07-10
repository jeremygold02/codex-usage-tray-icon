using System;
using System.Collections.Generic;

namespace CodexUsageTray
{
    internal sealed class UsageSnapshot
    {
        public DateTime LastUpdated;
        public DateTime LastAttempted;
        public LimitWindow FiveHour;
        public LimitWindow Weekly;
        public List<UsageLimitSet> AdditionalLimits = new List<UsageLimitSet>();
        public List<RateLimitResetCredit> AvailableResets = new List<RateLimitResetCredit>();
        public int? AvailableResetCount;
        public string PlanType;
        public string ErrorMessage;
        public string StatusMessage;
        public bool IsStale;
        public bool IsRefreshing;
        public bool IsPaused;

        public bool HasAnyLimit
        {
            get
            {
                if (Weekly != null || FiveHour != null)
                {
                    return true;
                }

                if (AdditionalLimits != null)
                {
                    for (int i = 0; i < AdditionalLimits.Count; i++)
                    {
                        UsageLimitSet limit = AdditionalLimits[i];
                        if (limit != null && (limit.FiveHour != null || limit.Weekly != null))
                        {
                            return true;
                        }
                    }
                }

                return false;
            }
        }

        public UsageSnapshot Clone()
        {
            UsageSnapshot clone = (UsageSnapshot)MemberwiseClone();
            clone.FiveHour = FiveHour != null ? FiveHour.Clone() : null;
            clone.Weekly = Weekly != null ? Weekly.Clone() : null;
            clone.AdditionalLimits = new List<UsageLimitSet>();
            clone.AvailableResets = new List<RateLimitResetCredit>();

            if (AdditionalLimits != null)
            {
                for (int i = 0; i < AdditionalLimits.Count; i++)
                {
                    UsageLimitSet limit = AdditionalLimits[i];
                    clone.AdditionalLimits.Add(limit != null ? limit.Clone() : null);
                }
            }

            if (AvailableResets != null)
            {
                for (int i = 0; i < AvailableResets.Count; i++)
                {
                    RateLimitResetCredit credit = AvailableResets[i];
                    clone.AvailableResets.Add(credit != null ? credit.Clone() : null);
                }
            }

            return clone;
        }

        public static UsageSnapshot FromError(string message)
        {
            UsageSnapshot snapshot = new UsageSnapshot();
            snapshot.LastAttempted = DateTime.Now;
            snapshot.ErrorMessage = message;
            return snapshot;
        }
    }
}
