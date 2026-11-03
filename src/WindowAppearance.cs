using System;
using System.Runtime.InteropServices;
using System.Windows.Forms;

namespace CodexUsageTray
{
    internal static class WindowAppearance
    {
        private const int UseImmersiveDarkMode = 20;
        private const int CaptionColor = 35;
        private const int CaptionTextColor = 36;
        private const int DefaultColor = -1;

        [DllImport("dwmapi.dll")]
        private static extern int DwmSetWindowAttribute(IntPtr window, int attribute, ref int value, int valueSize);

        public static void ApplyTitleBar(Form form, bool dark)
        {
            if (form == null || form.IsDisposed || !form.IsHandleCreated) return;
            int enabled = dark && !SystemInformation.HighContrast ? 1 : 0;
            int caption = enabled == 1 ? 0x1B1511 : DefaultColor;
            int text = enabled == 1 ? 0xFFFFFF : DefaultColor;
            try
            {
                DwmSetWindowAttribute(form.Handle, UseImmersiveDarkMode, ref enabled, sizeof(int));
                DwmSetWindowAttribute(form.Handle, CaptionColor, ref caption, sizeof(int));
                DwmSetWindowAttribute(form.Handle, CaptionTextColor, ref text, sizeof(int));
            }
            catch (DllNotFoundException) { }
            catch (EntryPointNotFoundException) { }
        }
    }
}
