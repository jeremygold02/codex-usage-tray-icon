using System;
using System.Diagnostics;

namespace CodexUsageTray
{
    internal static class CodexActivityMonitor
    {
        public static bool IsCodexRunning()
        {
            Process[] processes = Process.GetProcesses();
            try
            {
                foreach (Process process in processes)
                {
                    try
                    {
                        if (IsCodexProcessName(process.ProcessName))
                        {
                            return true;
                        }
                    }
                    catch
                    {
                    }
                }
            }
            finally
            {
                foreach (Process process in processes)
                {
                    process.Dispose();
                }
            }

            return false;
        }

        private static bool IsCodexProcessName(string processName)
        {
            return !string.IsNullOrEmpty(processName)
                && processName.StartsWith("codex", StringComparison.OrdinalIgnoreCase)
                && !string.Equals(processName, "CodexUsageTray", StringComparison.OrdinalIgnoreCase);
        }
    }
}
