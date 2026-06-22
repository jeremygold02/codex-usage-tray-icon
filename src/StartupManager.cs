using System;
using System.Windows.Forms;
using Microsoft.Win32;

namespace CodexUsageTray
{
    internal static class StartupManager
    {
        private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
        private const string ValueName = "CodexUsageTray";

        public static bool IsEnabled()
        {
            try
            {
                using (RegistryKey key = Registry.CurrentUser.OpenSubKey(RunKeyPath, false))
                {
                    string value = key != null ? key.GetValue(ValueName) as string : null;
                    return !string.IsNullOrWhiteSpace(value);
                }
            }
            catch
            {
                return false;
            }
        }

        public static void SetEnabled(bool enabled)
        {
            using (RegistryKey key = Registry.CurrentUser.CreateSubKey(RunKeyPath))
            {
                if (key == null)
                {
                    throw new InvalidOperationException("Could not open the Windows startup registry key.");
                }

                if (enabled)
                {
                    key.SetValue(ValueName, Quote(Application.ExecutablePath), RegistryValueKind.String);
                }
                else
                {
                    key.DeleteValue(ValueName, false);
                }
            }
        }

        private static string Quote(string path)
        {
            return "\"" + path + "\"";
        }
    }
}
