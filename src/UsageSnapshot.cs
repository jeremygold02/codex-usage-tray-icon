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
    internal sealed class UsageSnapshot
    {
        public LimitWindow Weekly;
        public LimitWindow FiveHour;
        public DateTime LastUpdated;
        public string ErrorMessage;

        public bool HasAnyLimit
        {
            get { return Weekly != null || FiveHour != null; }
        }

        public static UsageSnapshot FromError(string message)
        {
            UsageSnapshot snapshot = new UsageSnapshot();
            snapshot.LastUpdated = DateTime.Now;
            snapshot.ErrorMessage = message;
            return snapshot;
        }
    }
}
