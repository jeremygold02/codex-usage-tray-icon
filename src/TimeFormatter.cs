using System;
using System.Globalization;

namespace CodexUsageTray
{
    internal static class TimeFormatter
    {
        public static string FormatDuration(int totalSeconds)
        {
            if (totalSeconds < 0)
            {
                totalSeconds = 0;
            }

            TimeSpan span = TimeSpan.FromSeconds(totalSeconds);
            if (span.TotalDays >= 1)
            {
                return ((int)span.TotalDays) + "d " + span.Hours + "h";
            }
            if (span.TotalHours >= 1)
            {
                return ((int)span.TotalHours) + "h " + span.Minutes + "m";
            }
            if (span.TotalMinutes >= 1)
            {
                return ((int)span.TotalMinutes) + "m";
            }

            return span.Seconds + "s";
        }

        public static string FormatClock(DateTime value)
        {
            CultureInfo culture = CultureInfo.CurrentCulture;
            return value.ToString(culture.DateTimeFormat.ShortTimePattern, culture);
        }

        public static string FormatResetDateTime(DateTime lastUpdated, int resetAfterSeconds)
        {
            if (resetAfterSeconds < 0)
            {
                resetAfterSeconds = 0;
            }

            return FormatDateTime(lastUpdated.AddSeconds(resetAfterSeconds));
        }

        public static string FormatDateTime(DateTime value)
        {
            CultureInfo culture = CultureInfo.CurrentCulture;
            string datePattern = value.Year == DateTime.Now.Year
                ? "MMM d"
                : culture.DateTimeFormat.ShortDatePattern;
            string pattern = datePattern + ", " + culture.DateTimeFormat.ShortTimePattern;
            return value.ToString(pattern, culture);
        }
    }
}
