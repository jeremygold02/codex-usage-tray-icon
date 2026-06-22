using System;
using System.Collections.Generic;
using System.Collections.Specialized;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.IO;
using System.Net;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;

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
            return value.ToString("h:mm tt");
        }

        public static string FormatResetDateTime(DateTime lastUpdated, int resetAfterSeconds)
        {
            if (resetAfterSeconds < 0)
            {
                resetAfterSeconds = 0;
            }

            return lastUpdated.AddSeconds(resetAfterSeconds).ToString("MMM d, h:mm tt");
        }
    }
}
