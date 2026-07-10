using System;
using System.Diagnostics;

namespace CodexUsageTray
{
    internal static class CodexActivityMonitor
    {
        public static bool IsCodexRunning()
        {
            int currentProcessId;
            try
            {
                using (Process currentProcess = Process.GetCurrentProcess())
                {
                    currentProcessId = currentProcess.Id;
                }
            }
            catch
            {
                return false;
            }

            Process[] processes;
            try
            {
                processes = Process.GetProcesses();
            }
            catch
            {
                return false;
            }

            try
            {
                foreach (Process process in processes)
                {
                    try
                    {
                        if (process.Id != currentProcessId && IsCodexProcessName(process.ProcessName))
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
                    try
                    {
                        process.Dispose();
                    }
                    catch
                    {
                    }
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
