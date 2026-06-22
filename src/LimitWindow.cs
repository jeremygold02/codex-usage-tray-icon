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
    }
}
