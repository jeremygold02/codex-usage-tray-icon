using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Net;
using System.Text;
using System.Web.Script.Serialization;

namespace CodexUsageTray
{
    internal sealed class ClaudeQuotaResponse
    {
        internal int Status;
        internal string Body;
        internal int RetryAfterSeconds;
    }

    // The subscription endpoints used by Claude Code and OpenQuota. No model requests are sent.
    internal sealed class ClaudeQuotaClient
    {
        private const string UsageUrl = "https://api.anthropic.com/api/oauth/usage";
        private const string TokenUrl = "https://platform.claude.com/v1/oauth/token";
        private const string ClientId = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
        private const int MaxBytes = 65536;
        private static readonly DateTime Epoch = new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc);
        private readonly string credentialsPath;
        private readonly string cachePath;
        private readonly string fallbackPath;
        private readonly Func<string, string, string, ClaudeQuotaResponse> send;
        private readonly Func<DateTime> clock;
        private DateTime nextPoll;
        private DateTime rateLimitUntil;
        private string failure;
        private Dictionary<string, object> pendingOriginal;
        private Dictionary<string, object> pendingRotation;

        internal ClaudeQuotaClient() : this(DefaultCredentialsPath,
            Path.Combine(Path.GetDirectoryName(ClaudeUsageSource.DefaultPath), "claude-account-usage.json"),
            ClaudeUsageSource.DefaultPath, Send, delegate { return DateTime.UtcNow; }) { }

        internal ClaudeQuotaClient(string credentialsPath, string cachePath, string fallbackPath,
            Func<string, string, string, ClaudeQuotaResponse> send, Func<DateTime> clock)
        {
            this.credentialsPath = credentialsPath;
            this.cachePath = cachePath;
            this.fallbackPath = fallbackPath;
            this.send = send;
            this.clock = clock;
        }

        private static string DefaultCredentialsPath
        {
            get
            {
                string directory = Environment.GetEnvironmentVariable("CLAUDE_CONFIG_DIR");
                if (string.IsNullOrWhiteSpace(directory)) directory = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".claude");
                return Path.Combine(directory, ".credentials.json");
            }
        }

        internal UsageSnapshot Read(bool force, Action requestStarted = null)
        {
            DateTime now = clock();
            Dictionary<string, object> root;
            Dictionary<string, object> oauth;
            try
            {
                root = LoadCredentials();
                oauth = Object(root, "claudeAiOauth");
                if (string.IsNullOrWhiteSpace(Text(oauth, "accessToken"))) return null;
                if (pendingRotation != null)
                {
                    if (SameTokens(oauth, pendingOriginal))
                    {
                        try
                        {
                            oauth = PersistRotation(pendingOriginal, pendingRotation);
                            pendingRotation = pendingOriginal = null;
                        }
                        catch { oauth = pendingRotation; }
                    }
                    else pendingRotation = pendingOriginal = null;
                }
            }
            catch { return null; }

            if (now < rateLimitUntil || (!force && now < nextPoll))
            {
                UsageSnapshot saved = Cached(now);
                if (force && saved != null)
                    saved.StatusMessage = "Claude rate limited - retry at " +
                        rateLimitUntil.ToLocalTime().ToString("T", CultureInfo.CurrentCulture);
                return saved;
            }
            nextPoll = now.AddMinutes(5);
            rateLimitUntil = DateTime.MinValue;
            try
            {
                if (requestStarted != null) requestStarted();
                double expiry = Number(oauth, "expiresAt");
                if (expiry > 0 && expiry <= (now.AddMinutes(5) - Epoch).TotalMilliseconds)
                    oauth = Refresh(oauth, now);
                if (string.IsNullOrWhiteSpace(Text(oauth, "accessToken"))) return null;
                ClaudeQuotaResponse response = send(UsageUrl, Text(oauth, "accessToken"), null);
                if (response.Status == 401 || response.Status == 403)
                {
                    oauth = Refresh(oauth, now);
                    if (string.IsNullOrWhiteSpace(Text(oauth, "accessToken"))) return null;
                    response = send(UsageUrl, Text(oauth, "accessToken"), null);
                }
                if (response.Status != 200)
                {
                    Failed(response, now);
                    return Cached(now);
                }
                string input = ConvertUsage(response.Body, now);
                if (input == null) throw new InvalidDataException();
                if (!ClaudeUsageSource.CaptureStatusLine(input, cachePath, now)) throw new IOException();
                failure = null;
                UsageSnapshot updated = Cached(now);
                if (force && updated != null && string.IsNullOrEmpty(updated.StatusMessage))
                    updated.StatusMessage = "Claude usage updated";
                return updated;
            }
            catch (QuotaRequestException error)
            {
                Failed(error.Response, now);
                return Cached(now);
            }
            catch
            {
                Failed(null, now);
                return Cached(now);
            }
        }

        private void Failed(ClaudeQuotaResponse response, DateTime now)
        {
            int seconds = response != null && response.Status == 429
                ? Math.Max(300, response.RetryAfterSeconds) : 60;
            nextPoll = now.AddSeconds(seconds);
            rateLimitUntil = response != null && response.Status == 429 ? nextPoll : DateTime.MinValue;
            failure = response != null && (response.Status == 401 || response.Status == 403 || response.Status == 400)
                ? "Claude sign-in needs renewal - showing saved usage"
                : response != null && response.Status == 429
                    ? "Claude refresh rate limited - showing saved usage"
                    : "Claude refresh failed - showing saved usage";
        }

        private UsageSnapshot Cached(DateTime now)
        {
            UsageSnapshot saved = ClaudeUsageSource.ReadSnapshot(cachePath, now);
            UsageSnapshot fallback = ClaudeUsageSource.ReadSnapshot(fallbackPath, now);
            if (fallback != null && (saved == null || fallback.LastUpdated > saved.LastUpdated)) saved = fallback;
            if (saved != null && failure != null)
            {
                saved.IsStale = true;
                saved.StatusMessage = failure;
            }
            else if (saved != null && pendingRotation != null)
                saved.StatusMessage = "Claude login refresh could not be saved";
            return saved;
        }

        private Dictionary<string, object> LoadCredentials()
        {
            if (new FileInfo(credentialsPath).Length > MaxBytes) throw new InvalidDataException();
            return Parse(File.ReadAllText(credentialsPath));
        }

        private Dictionary<string, object> Refresh(Dictionary<string, object> oauth, DateTime now)
        {
            // Re-read before rotating: the CLI may already have renewed this generation.
            Dictionary<string, object> current = LoadCredentials();
            Dictionary<string, object> latest = Object(current, "claudeAiOauth");
            if (!SameTokens(latest, oauth) && !SameTokens(latest, pendingOriginal)) return latest;
            string refreshToken = Text(oauth, "refreshToken");
            if (string.IsNullOrWhiteSpace(refreshToken))
                throw new QuotaRequestException(new ClaudeQuotaResponse { Status = 401 });
            string body = new JavaScriptSerializer().Serialize(new Dictionary<string, object>
            {
                { "grant_type", "refresh_token" }, { "refresh_token", refreshToken },
                { "client_id", ClientId },
                { "scope", "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload" }
            });
            ClaudeQuotaResponse response = send(TokenUrl, null, body);
            if (response.Status != 200) throw new QuotaRequestException(response);
            Dictionary<string, object> tokens = Parse(response.Body);
            string accessToken = Text(tokens, "access_token");
            double expiresIn = Number(tokens, "expires_in");
            if (string.IsNullOrWhiteSpace(accessToken)) throw new InvalidDataException();

            Dictionary<string, object> replacement = new Dictionary<string, object>(oauth);
            replacement["accessToken"] = accessToken;
            string rotated = Text(tokens, "refresh_token");
            if (!string.IsNullOrWhiteSpace(rotated)) replacement["refreshToken"] = rotated;
            if (!double.IsNaN(expiresIn) && expiresIn > 0 && expiresIn <= 31536000)
                replacement["expiresAt"] = (long)(now.AddSeconds(expiresIn) - Epoch).TotalMilliseconds;
            else
                // Expiry is optional. Keep the rotated tokens and renew on a later 401.
                replacement.Remove("expiresAt");
            Dictionary<string, object> original = pendingOriginal ?? oauth;
            try
            {
                latest = PersistRotation(original, replacement);
                pendingOriginal = pendingRotation = null;
                return latest;
            }
            catch
            {
                // A rotated token can already invalidate the old token. Keep it in memory
                // and retry persistence, instead of rotating the obsolete token again.
                pendingOriginal = original;
                pendingRotation = replacement;
                return replacement;
            }
        }

        private Dictionary<string, object> PersistRotation(Dictionary<string, object> original,
            Dictionary<string, object> replacement)
        {
            Dictionary<string, object> root = LoadCredentials();
            Dictionary<string, object> latest = Object(root, "claudeAiOauth");
            if (!SameTokens(latest, original)) return latest;
            // Preserve other credential fields changed by the CLI while the request ran.
            Dictionary<string, object> merged = new Dictionary<string, object>(latest);
            merged["accessToken"] = replacement["accessToken"];
            merged["refreshToken"] = replacement["refreshToken"];
            if (replacement.ContainsKey("expiresAt")) merged["expiresAt"] = replacement["expiresAt"];
            else merged.Remove("expiresAt");
            root["claudeAiOauth"] = merged;
            string temporary = credentialsPath + "." + Guid.NewGuid().ToString("N") + ".tmp";
            try
            {
                using (FileStream stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                {
                    stream.SetAccessControl(File.GetAccessControl(credentialsPath));
                    byte[] bytes = Encoding.UTF8.GetBytes(new JavaScriptSerializer().Serialize(root));
                    stream.Write(bytes, 0, bytes.Length);
                    stream.Flush(true);
                }
                latest = Object(LoadCredentials(), "claudeAiOauth");
                if (!SameTokens(latest, original)) return latest;
                File.Replace(temporary, credentialsPath, null);
                return merged;
            }
            finally
            {
                try { if (File.Exists(temporary)) File.Delete(temporary); }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
        }

        private static bool SameTokens(Dictionary<string, object> left, Dictionary<string, object> right)
        {
            return left != null && right != null && Text(left, "accessToken") == Text(right, "accessToken") &&
                Text(left, "refreshToken") == Text(right, "refreshToken");
        }

        internal static string ConvertUsage(string json, DateTime now)
        {
            Dictionary<string, object> response = Parse(json);
            Dictionary<string, object> limits = new Dictionary<string, object>();
            foreach (string key in new[] { "five_hour", "seven_day" })
            {
                Dictionary<string, object> window = Object(response, key);
                double used = Number(window, "utilization");
                DateTimeOffset reset;
                if (double.IsNaN(used) || double.IsInfinity(used) || used < 0 || used > 100 ||
                    !DateTimeOffset.TryParse(Text(window, "resets_at"), CultureInfo.InvariantCulture,
                        DateTimeStyles.AssumeUniversal, out reset) || reset.UtcDateTime <= now) continue;
                limits[key] = new Dictionary<string, object>
                {
                    { "used_percentage", used },
                    { "resets_at", (long)(reset.UtcDateTime - Epoch).TotalSeconds }
                };
            }
            return limits.Count == 0 ? null : new JavaScriptSerializer().Serialize(
                new Dictionary<string, object> { { "rate_limits", limits } });
        }

        private static Dictionary<string, object> Parse(string json)
        {
            return new JavaScriptSerializer { MaxJsonLength = MaxBytes }.DeserializeObject(json)
                as Dictionary<string, object>;
        }
        private static Dictionary<string, object> Object(Dictionary<string, object> source, string key)
        {
            object value;
            return source != null && source.TryGetValue(key, out value) ? value as Dictionary<string, object> : null;
        }
        private static string Text(Dictionary<string, object> source, string key)
        {
            object value;
            return source != null && source.TryGetValue(key, out value) ? value as string : null;
        }
        private static double Number(Dictionary<string, object> source, string key)
        {
            object value;
            if (source == null || !source.TryGetValue(key, out value) || value == null || value is string || value is bool)
                return double.NaN;
            try { return Convert.ToDouble(value, CultureInfo.InvariantCulture); }
            catch { return double.NaN; }
        }

        private static ClaudeQuotaResponse Send(string url, string token, string body)
        {
            HttpWebRequest request = (HttpWebRequest)WebRequest.Create(url);
            request.Method = body == null ? "GET" : "POST";
            request.AllowAutoRedirect = false;
            request.Timeout = 10000;
            request.ReadWriteTimeout = 10000;
            request.Accept = "application/json";
            if (token != null)
            {
                request.Headers[HttpRequestHeader.Authorization] = "Bearer " + token;
                request.Headers["anthropic-beta"] = "oauth-2025-04-20";
            }
            if (body != null)
            {
                request.ContentType = "application/json";
                byte[] bytes = Encoding.UTF8.GetBytes(body);
                request.ContentLength = bytes.Length;
                using (Stream stream = request.GetRequestStream()) stream.Write(bytes, 0, bytes.Length);
            }
            HttpWebResponse response;
            try { response = (HttpWebResponse)request.GetResponse(); }
            catch (WebException error)
            {
                response = error.Response as HttpWebResponse;
                if (response == null) throw;
            }
            using (response)
            {
                int retry;
                if (!int.TryParse(response.Headers["Retry-After"], out retry))
                {
                    DateTimeOffset deadline;
                    retry = DateTimeOffset.TryParse(response.Headers["Retry-After"], out deadline)
                        ? (int)Math.Min(int.MaxValue, Math.Max(0,
                            Math.Ceiling((deadline.UtcDateTime - DateTime.UtcNow).TotalSeconds))) : 0;
                }
                using (StreamReader reader = new StreamReader(response.GetResponseStream()))
                {
                    char[] buffer = new char[MaxBytes + 1];
                    int count = reader.ReadBlock(buffer, 0, buffer.Length);
                    if (count > MaxBytes) throw new InvalidDataException();
                    return new ClaudeQuotaResponse { Status = (int)response.StatusCode,
                        Body = new string(buffer, 0, count), RetryAfterSeconds = retry };
                }
            }
        }

        private sealed class QuotaRequestException : Exception
        {
            internal readonly ClaudeQuotaResponse Response;
            internal QuotaRequestException(ClaudeQuotaResponse response) { Response = response; }
        }
    }
}
