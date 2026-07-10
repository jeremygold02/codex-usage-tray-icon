using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using Microsoft.Win32;

namespace CodexUsageTray
{
    internal static class StartupManager
    {
        private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
        private const string ValueName = "CodexUsageTray";

        [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CommandLineToArgvW(string commandLine, out int argumentCount);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr LocalFree(IntPtr memory);

        public static bool IsEnabled()
        {
            try
            {
                using (RegistryKey key = Registry.CurrentUser.OpenSubKey(RunKeyPath, false))
                {
                    string value = key != null ? key.GetValue(ValueName) as string : null;
                    return TargetsCurrentExecutable(value);
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

        private static bool TargetsCurrentExecutable(string command)
        {
            string target = GetExecutableTarget(command);
            if (string.IsNullOrWhiteSpace(target))
            {
                return false;
            }

            try
            {
                target = Environment.ExpandEnvironmentVariables(target);
                if (!Path.IsPathRooted(target))
                {
                    return false;
                }

                string registeredPath = Path.GetFullPath(target);
                string currentPath = Path.GetFullPath(Application.ExecutablePath);
                return string.Equals(registeredPath, currentPath, StringComparison.OrdinalIgnoreCase);
            }
            catch
            {
                return false;
            }
        }

        private static string GetExecutableTarget(string command)
        {
            if (string.IsNullOrWhiteSpace(command))
            {
                return null;
            }

            IntPtr arguments = IntPtr.Zero;
            try
            {
                int argumentCount;
                arguments = CommandLineToArgvW(command.Trim(), out argumentCount);
                if (arguments == IntPtr.Zero || argumentCount < 1)
                {
                    return null;
                }

                IntPtr firstArgument = Marshal.ReadIntPtr(arguments);
                return firstArgument != IntPtr.Zero ? Marshal.PtrToStringUni(firstArgument) : null;
            }
            catch
            {
                return null;
            }
            finally
            {
                if (arguments != IntPtr.Zero)
                {
                    LocalFree(arguments);
                }
            }
        }

        private static string Quote(string path)
        {
            return "\"" + path + "\"";
        }
    }
}
