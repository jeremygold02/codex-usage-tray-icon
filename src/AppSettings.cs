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
    internal sealed class AppSettings
    {
        public const int CurrentSettingsVersion = 5;
        public const string DefaultTrayBoxColor = "#0078D7";
        public const string DefaultTrayTextColor = "#FFFFFF";
        public const string IconMetricWeekly = "Weekly";
        public const string IconMetricFiveHour = "FiveHour";
        public const string ThemeSystem = "System Default";
        public const string ThemeDark = "Dark Mode";
        public const string ThemeLight = "Light Mode";

        public int SettingsVersion { get; set; }
        public bool OverlayNumber { get; set; }
        public int CriticalThreshold { get; set; }
        public int LowThreshold { get; set; }
        public string IconMetric { get; set; }
        public bool ColorBars { get; set; }
        public bool ShowPopupResetTimes { get; set; }
        public bool ShowPopupLastUpdated { get; set; }
        public bool StartWithWindows { get; set; }
        public bool ThresholdNotifications { get; set; }
        public int RefreshSeconds { get; set; }
        public int IdleRefreshSeconds { get; set; }
        public bool ShowTrayBox { get; set; }
        public bool UseCustomTrayBoxColor { get; set; }
        public string TrayBoxColor { get; set; }
        public string TrayTextColor { get; set; }
        public string Theme { get; set; }

        public AppSettings()
        {
            SettingsVersion = CurrentSettingsVersion;
            OverlayNumber = true;
            CriticalThreshold = 15;
            LowThreshold = 25;
            IconMetric = IconMetricWeekly;
            ColorBars = true;
            ShowPopupResetTimes = true;
            ShowPopupLastUpdated = true;
            StartWithWindows = false;
            ThresholdNotifications = false;
            RefreshSeconds = 300;
            IdleRefreshSeconds = 0;
            ShowTrayBox = true;
            UseCustomTrayBoxColor = false;
            TrayBoxColor = DefaultTrayBoxColor;
            TrayTextColor = DefaultTrayTextColor;
            Theme = ThemeSystem;
        }

        public static AppSettings Load()
        {
            try
            {
                string path = GetSettingsPath();
                if (File.Exists(path))
                {
                    string json = File.ReadAllText(path);
                    bool legacySettings = json.IndexOf("\"SettingsVersion\"", StringComparison.OrdinalIgnoreCase) < 0;
                    AppSettings settings = new JavaScriptSerializer().Deserialize<AppSettings>(json);
                    if (settings != null)
                    {
                        settings.Normalize();
                        if (legacySettings && string.Equals(settings.Theme, ThemeDark, StringComparison.OrdinalIgnoreCase))
                        {
                            settings.Theme = ThemeSystem;
                        }
                        settings.SettingsVersion = CurrentSettingsVersion;
                        return settings;
                    }
                }
            }
            catch
            {
            }

            return new AppSettings();
        }

        public void Save()
        {
            string path = GetSettingsPath();
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            SettingsVersion = CurrentSettingsVersion;
            File.WriteAllText(path, new JavaScriptSerializer().Serialize(this));
        }

        private void Normalize()
        {
            OverlayNumber = true;
            if (CriticalThreshold < 1 || CriticalThreshold > 99)
            {
                CriticalThreshold = 15;
            }
            if (LowThreshold < 1 || LowThreshold > 99)
            {
                LowThreshold = 25;
            }
            if (CriticalThreshold > LowThreshold)
            {
                CriticalThreshold = LowThreshold;
            }
            if (string.IsNullOrEmpty(IconMetric))
            {
                IconMetric = IconMetricWeekly;
            }
            if (RefreshSeconds < 30)
            {
                RefreshSeconds = 30;
            }
            else if (RefreshSeconds > 3600)
            {
                RefreshSeconds = 3600;
            }
            if (IdleRefreshSeconds < 0)
            {
                IdleRefreshSeconds = 0;
            }
            else if (IdleRefreshSeconds > 7200)
            {
                IdleRefreshSeconds = 7200;
            }
            else if (IdleRefreshSeconds > 0 && IdleRefreshSeconds < RefreshSeconds)
            {
                IdleRefreshSeconds = RefreshSeconds;
            }
            if (string.IsNullOrWhiteSpace(TrayBoxColor) || !IsColorValue(TrayBoxColor))
            {
                TrayBoxColor = DefaultTrayBoxColor;
            }
            if (string.IsNullOrWhiteSpace(TrayTextColor) || !IsColorValue(TrayTextColor))
            {
                TrayTextColor = DefaultTrayTextColor;
            }
            if (string.IsNullOrEmpty(Theme))
            {
                Theme = ThemeSystem;
            }
            else if (string.Equals(Theme, ThemeDark, StringComparison.OrdinalIgnoreCase))
            {
                Theme = ThemeDark;
            }
            else if (string.Equals(Theme, ThemeLight, StringComparison.OrdinalIgnoreCase))
            {
                Theme = ThemeLight;
            }
            else
            {
                Theme = ThemeSystem;
            }
        }

        public static bool IsDarkTheme(string theme)
        {
            if (string.Equals(theme, ThemeDark, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
            if (string.Equals(theme, ThemeLight, StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }

            return IsWindowsAppDarkMode();
        }

        public Color GetTrayBoxColor()
        {
            return ParseColor(TrayBoxColor, Color.FromArgb(0, 120, 215));
        }

        public Color GetTrayTextColor()
        {
            return ParseColor(TrayTextColor, Color.White);
        }

        public static string FormatColor(Color color)
        {
            return "#" + color.R.ToString("X2") + color.G.ToString("X2") + color.B.ToString("X2");
        }

        private static bool IsColorValue(string value)
        {
            try
            {
                ColorTranslator.FromHtml(value);
                return true;
            }
            catch
            {
                return false;
            }
        }

        private static Color ParseColor(string value, Color fallback)
        {
            try
            {
                return ColorTranslator.FromHtml(value);
            }
            catch
            {
                return fallback;
            }
        }

        private static bool IsWindowsAppDarkMode()
        {
            try
            {
                object value = Microsoft.Win32.Registry.GetValue(
                    @"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                    "AppsUseLightTheme",
                    1);
                if (value is int)
                {
                    return (int)value == 0;
                }
            }
            catch
            {
            }

            return false;
        }

        private static string GetSettingsPath()
        {
            return Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                "CodexUsageTray",
                "settings.json");
        }
    }
}
