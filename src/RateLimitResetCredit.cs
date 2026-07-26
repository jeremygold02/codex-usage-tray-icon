using System;

namespace CodexUsageTray
{
    internal sealed class RateLimitResetCredit
    {
        public string Id;
        public string ResetType;
        public string Title;
        public DateTime? ExpiresAtUtc;

        public RateLimitResetCredit Clone()
        {
            return (RateLimitResetCredit)MemberwiseClone();
        }
    }
}
