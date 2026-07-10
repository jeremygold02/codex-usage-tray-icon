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
    internal static class Program
    {
        private const string SingleInstanceMutexName = "CodexUsageTray.SingleInstance";
        private const string ExitRequestEventName = "Local\\CodexUsageTray.ExitRequest";
        private static readonly IntPtr DpiAwarenessContextPerMonitorAwareV2 = new IntPtr(-4);

        private enum ProcessDpiAwareness
        {
            ProcessDpiUnaware = 0,
            ProcessSystemDpiAware = 1,
            ProcessPerMonitorDpiAware = 2
        }

        [DllImport("user32.dll", EntryPoint = "SetProcessDpiAwarenessContext")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetProcessDpiAwarenessContext(IntPtr dpiContext);

        [DllImport("shcore.dll", EntryPoint = "SetProcessDpiAwareness")]
        private static extern int SetProcessDpiAwareness(ProcessDpiAwareness awareness);

        [DllImport("user32.dll", EntryPoint = "SetProcessDPIAware")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetProcessDpiAware();

        [STAThread]
        private static void Main()
        {
            string[] args = Environment.GetCommandLineArgs();
            bool exitRequested = HasArgument(args, "--exit");
            bool createdNew;
            using (EventWaitHandle exitSignal = new EventWaitHandle(
                false,
                EventResetMode.AutoReset,
                ExitRequestEventName))
            using (Mutex mutex = new Mutex(true, SingleInstanceMutexName, out createdNew))
            {
                if (!createdNew)
                {
                    if (exitRequested)
                    {
                        exitSignal.Set();
                    }
                    return;
                }
                if (exitRequested)
                {
                    return;
                }

                ServicePointManager.SecurityProtocol = (SecurityProtocolType)3072;
                EnableDpiAwareness();
                Application.EnableVisualStyles();
                Application.SetCompatibleTextRenderingDefault(false);
                TrayAppContext context = new TrayAppContext(args);
                RegisteredWaitHandle exitRegistration = ThreadPool.RegisterWaitForSingleObject(
                    exitSignal,
                    delegate { context.RequestExit(); },
                    null,
                    Timeout.Infinite,
                    false);
                try
                {
                    Application.Run(context);
                }
                finally
                {
                    exitRegistration.Unregister(null);
                }
            }
        }

        private static bool HasArgument(string[] args, string value)
        {
            if (args == null)
            {
                return false;
            }

            for (int index = 0; index < args.Length; index++)
            {
                if (string.Equals(args[index], value, StringComparison.OrdinalIgnoreCase))
                {
                    return true;
                }
            }

            return false;
        }

        private static void EnableDpiAwareness()
        {
            if (TrySetPerMonitorV2DpiAwareness())
            {
                return;
            }
            if (TrySetPerMonitorDpiAwareness())
            {
                return;
            }
            TrySetSystemDpiAwareness();
        }

        private static bool TrySetPerMonitorV2DpiAwareness()
        {
            try
            {
                return SetProcessDpiAwarenessContext(DpiAwarenessContextPerMonitorAwareV2);
            }
            catch (Exception)
            {
                return false;
            }
        }

        private static bool TrySetPerMonitorDpiAwareness()
        {
            try
            {
                return SetProcessDpiAwareness(ProcessDpiAwareness.ProcessPerMonitorDpiAware) == 0;
            }
            catch (Exception)
            {
                return false;
            }
        }

        private static bool TrySetSystemDpiAwareness()
        {
            try
            {
                return SetProcessDpiAware();
            }
            catch (Exception)
            {
                return false;
            }
        }
    }
}
