using System;
using System.Text.RegularExpressions;

namespace CodexUsageTray
{
    internal static class AppVersion
    {
        public const string AppName = "Codex Usage Tray";
        public const string BaseVersion = "0.1.0";
        public const string ReleaseAssetName = "CodexUsageTray.exe";

        private static readonly Regex VersionPattern = new Regex(@"\d+(?:\.\d+){0,2}", RegexOptions.Compiled);

        public static string Current
        {
            get { return CleanVersion(BuildVersion.Version); }
        }

        public static string GitHubOwner
        {
            get { return CleanPart(BuildVersion.GitHubOwner, "jeremygold02"); }
        }

        public static string GitHubRepo
        {
            get { return CleanPart(BuildVersion.GitHubRepo, "codex-usage-tray-icon"); }
        }

        public static string GitHubRepoUrl
        {
            get { return "https://github.com/" + GitHubOwner + "/" + GitHubRepo; }
        }

        public static string GitHubApiRepo
        {
            get { return "https://api.github.com/repos/" + GitHubOwner + "/" + GitHubRepo; }
        }

        public static string DisplayVersion(string value)
        {
            return CleanVersion(value);
        }

        public static int CompareVersions(string left, string right)
        {
            int[] leftParts = VersionParts(left);
            int[] rightParts = VersionParts(right);
            for (int i = 0; i < 3; i++)
            {
                int comparison = leftParts[i].CompareTo(rightParts[i]);
                if (comparison != 0)
                {
                    return comparison;
                }
            }

            return 0;
        }

        private static string CleanVersion(string value)
        {
            Match match = VersionPattern.Match(value ?? "");
            return match.Success ? match.Value : BaseVersion;
        }

        private static int[] VersionParts(string value)
        {
            string[] rawParts = CleanVersion(value).Split('.');
            int[] parts = new int[] { 0, 0, 0 };
            for (int i = 0; i < rawParts.Length && i < parts.Length; i++)
            {
                int parsed;
                if (int.TryParse(rawParts[i], out parsed))
                {
                    parts[i] = parsed;
                }
            }

            return parts;
        }

        private static string CleanPart(string value, string fallback)
        {
            return string.IsNullOrWhiteSpace(value) ? fallback : value.Trim();
        }
    }
}
