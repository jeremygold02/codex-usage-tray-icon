using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Text;
using System.Web.Script.Serialization;

namespace CodexUsageTray
{
    internal sealed class AppSettings
    {
        public const int CurrentSettingsVersion = 7;
        public const string DefaultTrayBoxColor = "#0078D7";
        public const string DefaultTrayTextColor = "#FFFFFF";
        public const string IconMetricWeekly = "Weekly";
        public const string IconMetricFiveHour = "FiveHour";
        public const string ThemeSystem = "System Default";
        public const string ThemeDark = "Dark Mode";
        public const string ThemeLight = "Light Mode";

        private bool saveBlockedByNewerVersion;
        private int protectedSettingsVersion;
        private string protectedSettingsPath;

        private enum SettingsFileResult
        {
            Missing,
            Invalid,
            Loaded,
            NewerVersion
        }

        public int SettingsVersion { get; set; }
        public bool OverlayNumber { get; set; }
        public int CriticalThreshold { get; set; }
        public int LowThreshold { get; set; }
        public string IconMetric { get; set; }
        public bool ColorBars { get; set; }
        public bool ShowPopupResetTimes { get; set; }
        public bool ShowPopupLastUpdated { get; set; }
        public bool ShowAdditionalLimits { get; set; }
        public bool ShowResetAvailability { get; set; }
        public bool StartWithWindows { get; set; }
        public bool ThresholdNotifications { get; set; }
        public bool AutoRedeemResetCredits { get; set; }
        public int AutoRedeemLeadMinutes { get; set; }
        public int RefreshSeconds { get; set; }
        public int IdleRefreshSeconds { get; set; }
        public bool ShowTrayBox { get; set; }
        public bool UseCustomTrayBoxColor { get; set; }
        public string TrayBoxColor { get; set; }
        public string TrayTextColor { get; set; }
        public string Theme { get; set; }
        public string AutoRedeemCreditId { get; set; }
        public string AutoRedeemIdempotencyKey { get; set; }
        public string AutoRedeemAttemptStatus { get; set; }
        public DateTime? AutoRedeemLastAttemptUtc { get; set; }

        internal bool IsSaveBlockedByNewerVersion
        {
            get { return saveBlockedByNewerVersion; }
        }

        internal string SaveBlockedMessage
        {
            get
            {
                return BuildNewerVersionMessage(protectedSettingsVersion, protectedSettingsPath);
            }
        }

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
            ShowAdditionalLimits = true;
            ShowResetAvailability = true;
            StartWithWindows = false;
            ThresholdNotifications = false;
            AutoRedeemResetCredits = false;
            AutoRedeemLeadMinutes = 5;
            RefreshSeconds = 300;
            IdleRefreshSeconds = 0;
            ShowTrayBox = true;
            UseCustomTrayBoxColor = false;
            TrayBoxColor = DefaultTrayBoxColor;
            TrayTextColor = DefaultTrayTextColor;
            Theme = ThemeSystem;
        }

        internal AppSettings Clone()
        {
            return (AppSettings)MemberwiseClone();
        }

        internal void CopyValuesFrom(AppSettings source)
        {
            if (source == null)
            {
                throw new ArgumentNullException("source");
            }

            SettingsVersion = source.SettingsVersion;
            OverlayNumber = source.OverlayNumber;
            CriticalThreshold = source.CriticalThreshold;
            LowThreshold = source.LowThreshold;
            IconMetric = source.IconMetric;
            ColorBars = source.ColorBars;
            ShowPopupResetTimes = source.ShowPopupResetTimes;
            ShowPopupLastUpdated = source.ShowPopupLastUpdated;
            ShowAdditionalLimits = source.ShowAdditionalLimits;
            ShowResetAvailability = source.ShowResetAvailability;
            StartWithWindows = source.StartWithWindows;
            ThresholdNotifications = source.ThresholdNotifications;
            AutoRedeemResetCredits = source.AutoRedeemResetCredits;
            AutoRedeemLeadMinutes = source.AutoRedeemLeadMinutes;
            RefreshSeconds = source.RefreshSeconds;
            IdleRefreshSeconds = source.IdleRefreshSeconds;
            ShowTrayBox = source.ShowTrayBox;
            UseCustomTrayBoxColor = source.UseCustomTrayBoxColor;
            TrayBoxColor = source.TrayBoxColor;
            TrayTextColor = source.TrayTextColor;
            Theme = source.Theme;
            AutoRedeemCreditId = source.AutoRedeemCreditId;
            AutoRedeemIdempotencyKey = source.AutoRedeemIdempotencyKey;
            AutoRedeemAttemptStatus = source.AutoRedeemAttemptStatus;
            AutoRedeemLastAttemptUtc = source.AutoRedeemLastAttemptUtc;
        }

        public static AppSettings Load()
        {
            return LoadFromPath(GetSettingsPath());
        }

        private static AppSettings LoadFromPath(string path)
        {
            AppSettings settings;
            SettingsFileResult primaryResult = TryLoadFile(path, out settings);
            if (primaryResult == SettingsFileResult.Loaded || primaryResult == SettingsFileResult.NewerVersion)
            {
                return settings;
            }

            string backupPath = GetBackupPath(path);
            SettingsFileResult backupResult = TryLoadFile(backupPath, out settings);
            if (backupResult == SettingsFileResult.Loaded || backupResult == SettingsFileResult.NewerVersion)
            {
                return settings;
            }

            return new AppSettings();
        }

        public void Save()
        {
            SaveToPath(GetSettingsPath());
        }

        private void SaveToPath(string path)
        {
            EnsureSaveDoesNotOverwriteNewerSettings(path);

            string directory = Path.GetDirectoryName(path);
            if (string.IsNullOrEmpty(directory))
            {
                throw new InvalidOperationException("Could not determine the settings folder.");
            }

            Directory.CreateDirectory(directory);
            SettingsVersion = CurrentSettingsVersion;
            string json = new JavaScriptSerializer().Serialize(this);
            string tempPath = path + "." + Guid.NewGuid().ToString("N") + ".tmp";

            try
            {
                WriteTempFile(tempPath, json);
                if (File.Exists(path))
                {
                    File.Replace(tempPath, path, GetBackupPath(path), true);
                }
                else
                {
                    File.Move(tempPath, path);
                }
            }
            finally
            {
                if (File.Exists(tempPath))
                {
                    File.Delete(tempPath);
                }
            }
        }

        private static SettingsFileResult TryLoadFile(string path, out AppSettings settings)
        {
            settings = null;
            if (!File.Exists(path))
            {
                return SettingsFileResult.Missing;
            }

            try
            {
                string json = File.ReadAllText(path);
                JavaScriptSerializer serializer = new JavaScriptSerializer();
                object rawSettings = serializer.DeserializeObject(json);
                IDictionary<string, object> values = rawSettings as IDictionary<string, object>;
                if (values == null)
                {
                    return SettingsFileResult.Invalid;
                }

                int sourceVersion;
                bool hasVersion = TryGetSettingsVersion(values, out sourceVersion);
                if (!hasVersion)
                {
                    sourceVersion = 0;
                }

                if (sourceVersion > CurrentSettingsVersion)
                {
                    try
                    {
                        settings = serializer.Deserialize<AppSettings>(json);
                    }
                    catch
                    {
                        settings = null;
                    }

                    if (settings == null)
                    {
                        settings = new AppSettings();
                    }
                    settings.Normalize(sourceVersion);
                    settings.SettingsVersion = sourceVersion;
                    settings.saveBlockedByNewerVersion = true;
                    settings.protectedSettingsVersion = sourceVersion;
                    settings.protectedSettingsPath = path;
                    return SettingsFileResult.NewerVersion;
                }

                settings = serializer.Deserialize<AppSettings>(json);
                if (settings == null)
                {
                    return SettingsFileResult.Invalid;
                }

                settings.Normalize(sourceVersion);
                if (!hasVersion && string.Equals(settings.Theme, ThemeDark, StringComparison.OrdinalIgnoreCase))
                {
                    settings.Theme = ThemeSystem;
                }
                settings.SettingsVersion = CurrentSettingsVersion;
                return SettingsFileResult.Loaded;
            }
            catch
            {
                settings = null;
                return SettingsFileResult.Invalid;
            }
        }

        private void EnsureSaveDoesNotOverwriteNewerSettings(string path)
        {
            if (saveBlockedByNewerVersion)
            {
                throw new InvalidOperationException(SaveBlockedMessage);
            }

            int version;
            if (TryReadSettingsVersion(path, out version) && version > CurrentSettingsVersion)
            {
                throw new InvalidOperationException(BuildNewerVersionMessage(version, path));
            }

            string backupPath = GetBackupPath(path);
            if (TryReadSettingsVersion(backupPath, out version) && version > CurrentSettingsVersion)
            {
                throw new InvalidOperationException(BuildNewerVersionMessage(version, backupPath));
            }
        }

        private static bool TryReadSettingsVersion(string path, out int version)
        {
            version = 0;
            if (!File.Exists(path))
            {
                return false;
            }

            try
            {
                JavaScriptSerializer serializer = new JavaScriptSerializer();
                object rawSettings = serializer.DeserializeObject(File.ReadAllText(path));
                IDictionary<string, object> values = rawSettings as IDictionary<string, object>;
                return values != null && TryGetSettingsVersion(values, out version);
            }
            catch
            {
                return false;
            }
        }

        private static bool TryGetSettingsVersion(IDictionary<string, object> values, out int version)
        {
            version = 0;
            object rawVersion;
            if (!TryGetValueIgnoreCase(values, "SettingsVersion", out rawVersion) || rawVersion == null)
            {
                return false;
            }

            try
            {
                version = Convert.ToInt32(rawVersion);
                return version >= 0;
            }
            catch
            {
                version = 0;
                return false;
            }
        }

        private static bool TryGetValueIgnoreCase(
            IDictionary<string, object> values,
            string name,
            out object value)
        {
            foreach (KeyValuePair<string, object> item in values)
            {
                if (string.Equals(item.Key, name, StringComparison.OrdinalIgnoreCase))
                {
                    value = item.Value;
                    return true;
                }
            }

            value = null;
            return false;
        }

        private static void WriteTempFile(string path, string contents)
        {
            byte[] data = new UTF8Encoding(false).GetBytes(contents);
            using (FileStream stream = new FileStream(
                path,
                FileMode.CreateNew,
                FileAccess.Write,
                FileShare.None,
                4096,
                FileOptions.WriteThrough))
            {
                stream.Write(data, 0, data.Length);
                stream.Flush(true);
            }
        }

        private static string BuildNewerVersionMessage(int version, string path)
        {
            return "Settings were created by a newer app version (schema "
                + version.ToString()
                + "). This version supports schema "
                + CurrentSettingsVersion.ToString()
                + " and will not overwrite the file: "
                + path;
        }

        private void Normalize(int sourceVersion)
        {
            OverlayNumber = true;
            if (sourceVersion < 6)
            {
                ShowAdditionalLimits = true;
                ShowResetAvailability = true;
            }
            if (sourceVersion < 7)
            {
                AutoRedeemResetCredits = false;
                AutoRedeemLeadMinutes = 5;
                AutoRedeemCreditId = null;
                AutoRedeemIdempotencyKey = null;
                AutoRedeemAttemptStatus = null;
                AutoRedeemLastAttemptUtc = null;
            }
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
            if (AutoRedeemLeadMinutes < 1 || AutoRedeemLeadMinutes > 120)
            {
                AutoRedeemLeadMinutes = 5;
            }
            if (string.IsNullOrWhiteSpace(AutoRedeemCreditId) ||
                string.IsNullOrWhiteSpace(AutoRedeemIdempotencyKey))
            {
                AutoRedeemCreditId = null;
                AutoRedeemIdempotencyKey = null;
                AutoRedeemAttemptStatus = null;
                AutoRedeemLastAttemptUtc = null;
            }
            else if (AutoRedeemLastAttemptUtc.HasValue)
            {
                DateTime attemptedAt = AutoRedeemLastAttemptUtc.Value;
                AutoRedeemLastAttemptUtc = attemptedAt.Kind == DateTimeKind.Utc
                    ? attemptedAt
                    : attemptedAt.Kind == DateTimeKind.Local
                        ? attemptedAt.ToUniversalTime()
                        : DateTime.SpecifyKind(attemptedAt, DateTimeKind.Utc);
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

        private static string GetBackupPath(string settingsPath)
        {
            return settingsPath + ".bak";
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
