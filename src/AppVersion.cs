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
            get
            {
#if WPF_UI
                // Release builds bake their version into BuildVersion. A local
                // build can identify the release it was built from without
                // modifying that tracked source file.
                if (string.Equals(BuildVersion.Version, BaseVersion, StringComparison.Ordinal))
                {
                    object[] metadata = typeof(AppVersion).Assembly.GetCustomAttributes(
                        typeof(System.Reflection.AssemblyMetadataAttribute), false);
                    foreach (object item in metadata)
                    {
                        System.Reflection.AssemblyMetadataAttribute attribute =
                            item as System.Reflection.AssemblyMetadataAttribute;
                        if (attribute != null && attribute.Key == "LocalWpfVersion" &&
                            VersionPattern.IsMatch(attribute.Value ?? ""))
                        {
                            return CleanVersion(attribute.Value);
                        }
                    }
                }
#endif
                return CleanVersion(BuildVersion.Version);
            }
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
