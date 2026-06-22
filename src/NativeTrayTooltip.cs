using System;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal static class NativeTrayTooltip
    {
        private const int NimModify = 0x00000001;
        private const int NifTip = 0x00000004;
        private const int MaxTipLength = 127;

        [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool Shell_NotifyIcon(int dwMessage, ref NotifyIconData lpData);

        public static bool TrySetText(NotifyIcon notifyIcon, string text)
        {
            if (notifyIcon == null)
            {
                return false;
            }

            IntPtr handle;
            int id;
            if (!TryGetNativeIdentity(notifyIcon, out handle, out id) || handle == IntPtr.Zero)
            {
                return false;
            }

            NotifyIconData data = new NotifyIconData();
            data.cbSize = Marshal.SizeOf(typeof(NotifyIconData));
            data.hWnd = handle;
            data.uID = id;
            data.uFlags = NifTip;
            data.szTip = LimitTip(text);

            return Shell_NotifyIcon(NimModify, ref data);
        }

        private static bool TryGetNativeIdentity(NotifyIcon notifyIcon, out IntPtr handle, out int id)
        {
            handle = IntPtr.Zero;
            id = 0;

            try
            {
                Type type = typeof(NotifyIcon);
                FieldInfo windowField = type.GetField("window", BindingFlags.Instance | BindingFlags.NonPublic);
                FieldInfo idField = type.GetField("id", BindingFlags.Instance | BindingFlags.NonPublic);
                if (windowField == null || idField == null)
                {
                    return false;
                }

                object window = windowField.GetValue(notifyIcon);
                NativeWindow nativeWindow = window as NativeWindow;
                if (nativeWindow == null)
                {
                    return false;
                }

                handle = nativeWindow.Handle;
                id = (int)idField.GetValue(notifyIcon);
                return true;
            }
            catch
            {
                handle = IntPtr.Zero;
                id = 0;
                return false;
            }
        }

        private static string LimitTip(string text)
        {
            if (string.IsNullOrEmpty(text))
            {
                return "Codex Usage";
            }

            return text.Length > MaxTipLength ? text.Substring(0, MaxTipLength) : text;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct NotifyIconData
        {
            public int cbSize;
            public IntPtr hWnd;
            public int uID;
            public int uFlags;
            public int uCallbackMessage;
            public IntPtr hIcon;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)]
            public string szTip;
            public int dwState;
            public int dwStateMask;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
            public string szInfo;
            public int uTimeoutOrVersion;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)]
            public string szInfoTitle;
            public int dwInfoFlags;
            public Guid guidItem;
            public IntPtr hBalloonIcon;
        }
    }
}
