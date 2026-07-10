namespace CodexUsageTray
{
    internal sealed class UsageLimitSet
    {
        public string LimitId;
        public string DisplayName;
        public LimitWindow FiveHour;
        public LimitWindow Weekly;

        public UsageLimitSet Clone()
        {
            UsageLimitSet clone = (UsageLimitSet)MemberwiseClone();
            clone.FiveHour = FiveHour != null ? FiveHour.Clone() : null;
            clone.Weekly = Weekly != null ? Weekly.Clone() : null;
            return clone;
        }
    }
}
