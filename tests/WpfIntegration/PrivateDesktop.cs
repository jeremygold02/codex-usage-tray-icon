using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace CodexUsageTray
{
    internal static class PrivateDesktop
    {
        private const string Prefix = "CodexWpfCapture_";

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct StartupInfo
        {
            public int Size;
            public string Reserved, Desktop, Title;
            public int X, Y, XSize, YSize, XCount, YCount, Fill, Flags;
            public short Show, Reserved2Length;
            public IntPtr Reserved2, StdIn, StdOut, StdErr;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct ProcessInfo { public IntPtr Process, Thread; public int ProcessId, ThreadId; }

        public static int Launch(string executable, string output)
        {
            return LaunchCommand(executable, "--capture \"" + output + "\"", output);
        }

        public static int LaunchLegacy(string executable, string testExecutable, string fixture, string output)
        {
            return LaunchCommand(executable, "--legacy-child \"" + testExecutable + "\" \"" + fixture + "\" \"" + output + "\"", output);
        }

        private static int LaunchCommand(string executable, string arguments, string output)
        {
            string desktopName = Prefix + Guid.NewGuid().ToString("N");
            IntPtr desktop = CreateDesktop(desktopName, IntPtr.Zero, IntPtr.Zero, 0, 0x01ff, IntPtr.Zero);
            if (desktop == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            try
            {
                var startup = new StartupInfo { Size = Marshal.SizeOf(typeof(StartupInfo)), Desktop = desktopName };
                var command = new StringBuilder("\"" + executable + "\" " + arguments);
                ProcessInfo process;
                if (!CreateProcess(null, command, IntPtr.Zero, IntPtr.Zero, false, 0x08000000,
                    IntPtr.Zero, null, ref startup, out process)) throw new Win32Exception(Marshal.GetLastWin32Error());
                try
                {
                    if (WaitForSingleObject(process.Process, 120000) != 0)
                    {
                        TerminateProcess(process.Process, 1);
                        throw new TimeoutException("Private-desktop checks exceeded two minutes.");
                    }
                    uint code;
                    if (!GetExitCodeProcess(process.Process, out code)) throw new Win32Exception(Marshal.GetLastWin32Error());
                    string error = Path.Combine(output, "error.txt");
                    if (code != 0 && File.Exists(error)) Console.Error.WriteLine(File.ReadAllText(error));
                    string legacyLog = Path.Combine(output, "legacy-log.txt");
                    if (File.Exists(legacyLog)) Console.WriteLine(File.ReadAllText(legacyLog));
                    if (code == 0) Console.WriteLine("Private-desktop checks passed; captures: " + output);
                    return unchecked((int)code);
                }
                finally { CloseHandle(process.Thread); CloseHandle(process.Process); }
            }
            finally { CloseDesktop(desktop); }
        }

        public static void RequirePrivateDesktop()
        {
            string current = NameOf(GetThreadDesktop(GetCurrentThreadId()));
            if (!current.StartsWith(Prefix, StringComparison.Ordinal))
                throw new InvalidOperationException("Checks must run on a dedicated non-visible desktop.");
            IntPtr input = OpenInputDesktop(0, false, 1);
            if (input == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            try
            {
                if (string.Equals(current, NameOf(input), StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException("Checks must not run on the active desktop.");
            }
            finally { CloseDesktop(input); }
        }

        private static string NameOf(IntPtr desktop)
        {
            if (desktop == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            var name = new StringBuilder(256);
            int needed;
            if (!GetUserObjectInformation(desktop, 2, name, name.Capacity * sizeof(char), out needed))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return name.ToString();
        }

        [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
        [DllImport("user32.dll", SetLastError = true)] private static extern IntPtr GetThreadDesktop(uint thread);
        [DllImport("user32.dll", SetLastError = true)] private static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool GetUserObjectInformation(IntPtr handle, int index, StringBuilder data, int length, out int needed);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CreateDesktop(string name, IntPtr device, IntPtr mode, int flags, uint access, IntPtr security);
        [DllImport("user32.dll")] private static extern bool CloseDesktop(IntPtr desktop);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool CreateProcess(string application, StringBuilder command, IntPtr processAttributes,
            IntPtr threadAttributes, bool inherit, int flags, IntPtr environment, string directory,
            ref StartupInfo startup, out ProcessInfo process);
        [DllImport("kernel32.dll")] private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
        [DllImport("kernel32.dll")] private static extern bool GetExitCodeProcess(IntPtr handle, out uint code);
        [DllImport("kernel32.dll")] private static extern bool TerminateProcess(IntPtr handle, uint code);
        [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);
    }
}
