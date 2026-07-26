using System;

namespace CodexUsageTray
{
    internal static class AutomaticResetRedemptionPolicy
    {
        public static RateLimitResetCredit FindEligibleCredit(
            UsageSnapshot snapshot,
            DateTime nowUtc,
            int leadMinutes,
            string excludedCreditId)
        {
            if (!HasUsageToReset(snapshot))
            {
                return null;
            }

            DateTime normalizedNow = NormalizeUtc(nowUtc);
            DateTime deadline = normalizedNow.AddMinutes(NormalizeLeadMinutes(leadMinutes));
            RateLimitResetCredit selected = null;
            DateTime selectedExpiration = DateTime.MaxValue;

            if (snapshot == null || snapshot.AvailableResets == null)
            {
                return null;
            }

            for (int index = 0; index < snapshot.AvailableResets.Count; index++)
            {
                RateLimitResetCredit credit = snapshot.AvailableResets[index];
                DateTime expiration;
                if (!TryGetUsableExpiration(credit, normalizedNow, excludedCreditId, out expiration) ||
                    expiration > deadline ||
                    expiration >= selectedExpiration)
                {
                    continue;
                }

                selected = credit;
                selectedExpiration = expiration;
            }

            return selected;
        }

        public static DateTime? GetNextCheckUtc(
            UsageSnapshot snapshot,
            DateTime nowUtc,
            int leadMinutes,
            string excludedCreditId)
        {
            if (snapshot == null || snapshot.AvailableResets == null)
            {
                return null;
            }

            DateTime normalizedNow = NormalizeUtc(nowUtc);
            TimeSpan lead = TimeSpan.FromMinutes(NormalizeLeadMinutes(leadMinutes));
            DateTime? nextCheck = null;

            for (int index = 0; index < snapshot.AvailableResets.Count; index++)
            {
                DateTime expiration;
                if (!TryGetUsableExpiration(
                    snapshot.AvailableResets[index],
                    normalizedNow,
                    excludedCreditId,
                    out expiration))
                {
                    continue;
                }

                DateTime checkAt = expiration - lead;
                if (checkAt < normalizedNow)
                {
                    checkAt = normalizedNow;
                }
                if (!nextCheck.HasValue || checkAt < nextCheck.Value)
                {
                    nextCheck = checkAt;
                }
            }

            return nextCheck;
        }

        public static bool HasUsageToReset(UsageSnapshot snapshot)
        {
            return snapshot != null &&
                (HasUsage(snapshot.Weekly) || HasUsage(snapshot.FiveHour));
        }

        private static bool HasUsage(LimitWindow window)
        {
            return window != null && window.UsedPercent > 0.0;
        }

        private static bool TryGetUsableExpiration(
            RateLimitResetCredit credit,
            DateTime nowUtc,
            string excludedCreditId,
            out DateTime expirationUtc)
        {
            expirationUtc = DateTime.MinValue;
            if (credit == null ||
                string.IsNullOrWhiteSpace(credit.Id) ||
                !credit.ExpiresAtUtc.HasValue ||
                !IsSupportedResetType(credit.ResetType) ||
                (!string.IsNullOrEmpty(excludedCreditId) &&
                    string.Equals(credit.Id, excludedCreditId, StringComparison.Ordinal)))
            {
                return false;
            }

            expirationUtc = NormalizeUtc(credit.ExpiresAtUtc.Value);
            return expirationUtc > nowUtc;
        }

        private static bool IsSupportedResetType(string resetType)
        {
            if (string.IsNullOrWhiteSpace(resetType))
            {
                return false;
            }

            string normalized = resetType.Replace("_", "").Replace("-", "");
            return string.Equals(
                normalized,
                "codexRateLimits",
                StringComparison.OrdinalIgnoreCase);
        }

        private static int NormalizeLeadMinutes(int leadMinutes)
        {
            return Math.Max(1, Math.Min(120, leadMinutes));
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
