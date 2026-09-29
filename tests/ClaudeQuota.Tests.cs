using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Web.Script.Serialization;

namespace CodexUsageTray.Tests
{
    internal static class ClaudeQuotaTests
    {
        public static void Run()
        {
            TestPolling();
            TestRateLimitBackoff();
            TestFailedRefreshKeepsObservation();
            TestTokenRefresh(false);
            TestTokenRefresh(true);
            TestConcurrentCredentialRotation();
            TestCredentialPersistenceRetry();
            TestRefreshWithoutExpiry();
            TestMissingCredentials();
            TestWindowValidation();
        }

        private static void TestPolling()
        {
            using (Fixture test = new Fixture())
            {
                test.WriteCredentials("original", "refresh", test.Now.AddHours(1));
                int started = 0;
                Action start = delegate { started++; };
                Assert(test.Client.Read(false, start) != null && test.Requests.Count == 1 && started == 1,
                    "first read fetches usage and announces the request");
                UsageSnapshot refreshed = test.Client.Read(true, start);
                Assert(test.Requests.Count == 2 && started == 2 && refreshed.StatusMessage == "Claude usage updated",
                    "manual refresh can run immediately after completion and confirms success");
                test.Now = test.Now.AddMinutes(5).AddTicks(-1);
                test.Client.Read(false);
                Assert(test.Requests.Count == 2, "automatic polling waits five minutes after manual refresh");
                test.Now = test.Now.AddTicks(1);
                test.Client.Read(false);
                Assert(test.Requests.Count == 3, "automatic polling resumes at five minutes");
                Assert(test.Requests[0].Url.EndsWith("/api/oauth/usage") &&
                    test.Requests[0].Token == "original" && test.Requests[0].Body == null,
                    "usage polling sends the access token without a request body");
            }
        }

        private static void TestRateLimitBackoff()
        {
            foreach (int retryAfter in new[] { 0, 900, 90000 })
            using (Fixture test = new Fixture())
            {
                test.WriteCredentials("original", "refresh", test.Now.AddDays(7));
                test.Respond = delegate { return new ClaudeQuotaResponse { Status = 429, RetryAfterSeconds = retryAfter }; };
                test.Client.Read(false);
                test.Now = test.Now.AddSeconds(Math.Max(300, retryAfter)).AddTicks(-1);
                test.Client.Read(false);
                UsageSnapshot limited = test.Client.Read(true);
                Assert(test.Requests.Count == 1 && limited == null, "automatic and forced reads honor rate limit backoff");
                test.Now = test.Now.AddTicks(1);
                test.Respond = delegate { return test.Success(); };
                Assert(test.Client.Read(true) != null && test.Requests.Count == 2,
                    "refresh resumes at the rate limit deadline");
            }
        }

        private static void TestFailedRefreshKeepsObservation()
        {
            using (Fixture test = new Fixture())
            {
                test.WriteCredentials("original", "refresh", test.Now.AddHours(1));
                DateTime observed = test.Now;
                test.Client.Read(false);
                string stored = File.ReadAllText(test.CachePath);
                test.Now = test.Now.AddMinutes(5);
                test.Respond = delegate { return new ClaudeQuotaResponse { Status = 503 }; };
                UsageSnapshot failed = test.Client.Read(false);
                Assert(failed != null && failed.IsStale && failed.LastUpdated.ToUniversalTime() == observed &&
                    failed.FiveHour.UsedPercent == 25 && failed.StatusMessage.Contains("failed"),
                    "failed requests retain the old reading and observation time with a stale status");
                Assert(File.ReadAllText(test.CachePath) == stored, "failure does not rewrite the saved observation");
                test.Now = test.Now.AddSeconds(59);
                test.Client.Read(false);
                Assert(test.Requests.Count == 2, "automatic refresh respects ordinary failure backoff");
                test.Respond = delegate { return test.Success(); };
                UsageSnapshot recovered = test.Client.Read(true);
                Assert(test.Requests.Count == 3 && recovered != null && !recovered.IsStale &&
                    recovered.LastUpdated.ToUniversalTime() == test.Now,
                    "manual retry bypasses ordinary failure backoff and restores a fresh observation");
                test.Now = test.Now.AddMinutes(5);
                test.Respond = delegate { return new ClaudeQuotaResponse { Status = 200, Body = "not json" }; };
                UsageSnapshot malformed = test.Client.Read(false);
                Assert(malformed != null && malformed.IsStale && malformed.LastUpdated == recovered.LastUpdated,
                    "malformed server data cannot replace the last valid observation");
                test.Respond = delegate { return new ClaudeQuotaResponse { Status = 429, RetryAfterSeconds = 900 }; };
                test.Client.Read(true);
                int requestsBeforeCooldown = test.Requests.Count;
                UsageSnapshot limited = test.Client.Read(true, delegate { throw new InvalidOperationException("Unexpected request"); });
                Assert(test.Requests.Count == requestsBeforeCooldown && limited != null && limited.IsStale &&
                    limited.StatusMessage.StartsWith("Claude rate limited - retry at "),
                    "server cooldown explains why manual refresh waits and retains saved usage");
            }
        }

        private static void TestTokenRefresh(bool unauthorized)
        {
            using (Fixture test = new Fixture())
            {
                DateTime refreshedAt = test.Now;
                test.WriteCredentials("original", "refresh", test.Now.AddMinutes(unauthorized ? 60 : 5));
                test.Respond = delegate(Request request)
                {
                    if (request.Body != null)
                        return new ClaudeQuotaResponse { Status = 200,
                            Body = "{\"access_token\":\"rotated-access\",\"refresh_token\":\"rotated-refresh\",\"expires_in\":3600}" };
                    if (unauthorized && request.Token == "original") return new ClaudeQuotaResponse { Status = 401 };
                    return test.Success();
                };
                UsageSnapshot snapshot = test.Client.Read(false);
                Assert(snapshot != null && !snapshot.IsStale, "token refresh recovers account usage");
                Assert(test.Requests.Count == (unauthorized ? 3 : 2),
                    "expiry refresh precedes usage and unauthorized refresh retries usage once");
                Request refresh = test.Requests[unauthorized ? 1 : 0];
                Dictionary<string, object> requestBody = Parse(refresh.Body);
                Assert(refresh.Url.EndsWith("/v1/oauth/token") && refresh.Token == null &&
                    (string)requestBody["grant_type"] == "refresh_token" &&
                    (string)requestBody["refresh_token"] == "refresh", "refresh uses the existing refresh token");
                Assert(test.Requests[test.Requests.Count - 1].Token == "rotated-access",
                    "usage retry uses the rotated access token");
                Dictionary<string, object> saved = Parse(File.ReadAllText(test.CredentialsPath));
                Dictionary<string, object> oauth = (Dictionary<string, object>)saved["claudeAiOauth"];
                Assert((string)oauth["accessToken"] == "rotated-access" &&
                    (string)oauth["refreshToken"] == "rotated-refresh" &&
                    Convert.ToInt64(oauth["expiresAt"]) == UnixMilliseconds(refreshedAt.AddHours(1)),
                    "rotated tokens and expiry are saved");
                Assert((string)saved["unrelatedSetting"] == "keep-root" &&
                    (string)oauth["subscriptionType"] == "max" &&
                    ((object[])oauth["scopes"]).Length == 2,
                    "refresh preserves unrelated root and OAuth fields");
            }
        }

        private static void TestConcurrentCredentialRotation()
        {
            using (Fixture test = new Fixture())
            {
                test.WriteCredentials("original", "refresh", test.Now);
                string concurrent = null;
                test.Respond = delegate(Request request)
                {
                    if (request.Body == null) return test.Success();
                    test.WriteCredentials("cli-access", "cli-refresh", test.Now.AddHours(2));
                    concurrent = File.ReadAllText(test.CredentialsPath);
                    return new ClaudeQuotaResponse { Status = 200,
                        Body = "{\"access_token\":\"discard-access\",\"refresh_token\":\"discard-refresh\",\"expires_in\":3600}" };
                };
                Assert(test.Client.Read(false) != null && test.Requests.Count == 2,
                    "usage remains available after a concurrent CLI refresh");
                Assert(File.ReadAllText(test.CredentialsPath) == concurrent && test.Requests[1].Token == "cli-access",
                    "a credential generation replaced during refresh is preserved and used");
            }
        }

        private static void TestCredentialPersistenceRetry()
        {
            using (Fixture test = new Fixture())
            {
                test.WriteCredentials("original", "refresh", test.Now);
                FileStream credentialLock = null;
                test.Respond = delegate(Request request)
                {
                    if (request.Body == null) return test.Success();
                    credentialLock = new FileStream(test.CredentialsPath, FileMode.Open, FileAccess.Read, FileShare.Read);
                    return new ClaudeQuotaResponse { Status = 200,
                        Body = "{\"access_token\":\"rotated-access\",\"refresh_token\":\"rotated-refresh\",\"expires_in\":3600}" };
                };
                try
                {
                    UsageSnapshot snapshot = test.Client.Read(false);
                    Assert(snapshot != null && !snapshot.IsStale && test.Requests.Count == 2 &&
                        test.Requests[1].Token == "rotated-access", "a save failure still uses newly rotated credentials");
                    Assert(snapshot.StatusMessage.Contains("could not be saved"), "a pending credential save is visible");
                    Dictionary<string, object> oauth = (Dictionary<string, object>)Parse(
                        File.ReadAllText(test.CredentialsPath))["claudeAiOauth"];
                    Assert((string)oauth["accessToken"] == "original", "locked credentials stay unchanged");
                }
                finally
                {
                    if (credentialLock != null) credentialLock.Dispose();
                }
                test.Client.Read(false);
                Dictionary<string, object> saved = (Dictionary<string, object>)Parse(
                    File.ReadAllText(test.CredentialsPath))["claudeAiOauth"];
                Assert((string)saved["accessToken"] == "rotated-access" &&
                    (string)saved["refreshToken"] == "rotated-refresh" && test.Requests.Count == 2,
                    "next read saves the pending rotation without another token request");
            }
        }

        private static void TestRefreshWithoutExpiry()
        {
            using (Fixture test = new Fixture())
            {
                test.WriteCredentials("original", "refresh", test.Now);
                test.Respond = delegate(Request request)
                {
                    return request.Body == null ? test.Success() : new ClaudeQuotaResponse { Status = 200,
                        Body = "{\"access_token\":\"rotated-access\",\"refresh_token\":\"rotated-refresh\"}" };
                };
                Assert(test.Client.Read(false) != null, "refresh accepts a response without optional expiry");
                Dictionary<string, object> oauth = (Dictionary<string, object>)Parse(
                    File.ReadAllText(test.CredentialsPath))["claudeAiOauth"];
                Assert((string)oauth["accessToken"] == "rotated-access" &&
                    (string)oauth["refreshToken"] == "rotated-refresh" && !oauth.ContainsKey("expiresAt"),
                    "rotated credentials are saved without retaining the obsolete expiry");
                test.Now = test.Now.AddSeconds(30);
                Assert(test.Client.Read(true) != null && test.Requests.Count == 3 &&
                    test.Requests[2].Body == null && test.Requests[2].Token == "rotated-access",
                    "missing expiry does not cause another token rotation on the next poll");
            }
        }

        private static void TestMissingCredentials()
        {
            using (Fixture test = new Fixture())
            {
                Assert(test.Client.Read(false) == null, "missing credentials return no account reading");
                foreach (string json in new[] { "not json", "{}", "{\"claudeAiOauth\":{\"accessToken\":\" \"}}" })
                {
                    File.WriteAllText(test.CredentialsPath, json);
                    Assert(test.Client.Read(true) == null, "invalid credentials return no account reading");
                }
                Assert(test.Requests.Count == 0, "missing or invalid credentials never issue HTTP requests");
            }
        }

        private static void TestWindowValidation()
        {
            using (Fixture test = new Fixture())
            {
                string reset = test.Now.AddHours(2).ToString("o", CultureInfo.InvariantCulture);
                foreach (string value in new[] { "null", "true", "\"20\"", "-1", "100.01", "{}", "[]" })
                {
                    string input = "{\"five_hour\":{\"utilization\":" + value + ",\"resets_at\":\"" + reset +
                        "\"},\"seven_day\":{\"utilization\":75,\"resets_at\":\"" + reset + "\"}}";
                    Dictionary<string, object> limits = ConvertedWindows(input, test.Now);
                    Assert(limits.Count == 1 && limits.ContainsKey("seven_day") &&
                        Convert.ToDouble(((Dictionary<string, object>)limits["seven_day"])["used_percentage"]) == 75,
                        "invalid utilization cannot hide a valid independent weekly window");
                }
                foreach (string invalidReset in new[] { "not-a-date", test.Now.ToString("o"), test.Now.AddSeconds(-1).ToString("o") })
                    Assert(ClaudeQuotaClient.ConvertUsage("{\"five_hour\":{\"utilization\":25,\"resets_at\":\"" +
                        invalidReset + "\"}}", test.Now) == null, "invalid or expired resets are ignored");
                foreach (string input in new[] { "{}", "null", "[]", "{\"five_hour\":null}",
                    "{\"five_hour\":{\"resets_at\":\"" + reset + "\"}}",
                    "{\"five_hour\":{\"utilization\":25}}" })
                    Assert(ClaudeQuotaClient.ConvertUsage(input, test.Now) == null, "missing quota fields provide no reading");
                Dictionary<string, object> bounds = ConvertedWindows("{\"five_hour\":{\"utilization\":0,\"resets_at\":\"" + reset +
                    "\"},\"seven_day\":{\"utilization\":100,\"resets_at\":\"" + reset + "\"}}", test.Now);
                Assert(Convert.ToDouble(((Dictionary<string, object>)bounds["five_hour"])["used_percentage"]) == 0 &&
                    Convert.ToDouble(((Dictionary<string, object>)bounds["seven_day"])["used_percentage"]) == 100,
                    "zero and one hundred percent are valid quota boundaries");
            }
        }

        private sealed class Request
        {
            internal string Url;
            internal string Token;
            internal string Body;
        }

        private sealed class Fixture : IDisposable
        {
            private readonly string directory = Path.Combine(Path.GetTempPath(), "ClaudeQuotaTests-" + Guid.NewGuid().ToString("N"));
            internal readonly string CredentialsPath;
            internal readonly string CachePath;
            internal readonly ClaudeQuotaClient Client;
            internal readonly List<Request> Requests = new List<Request>();
            internal DateTime Now = new DateTime(2026, 9, 28, 12, 0, 0, DateTimeKind.Utc);
            internal Func<Request, ClaudeQuotaResponse> Respond;

            internal Fixture()
            {
                Directory.CreateDirectory(directory);
                CredentialsPath = Path.Combine(directory, "credentials.json");
                CachePath = Path.Combine(directory, "account.json");
                Respond = delegate { return Success(); };
                Client = new ClaudeQuotaClient(CredentialsPath, CachePath, Path.Combine(directory, "fallback.json"),
                    delegate(string url, string token, string body)
                    {
                        Request request = new Request { Url = url, Token = token, Body = body };
                        Requests.Add(request);
                        return Respond(request);
                    }, delegate { return Now; });
            }

            internal void WriteCredentials(string accessToken, string refreshToken, DateTime expiry)
            {
                File.WriteAllText(CredentialsPath, new JavaScriptSerializer().Serialize(new Dictionary<string, object>
                {
                    { "unrelatedSetting", "keep-root" },
                    { "claudeAiOauth", new Dictionary<string, object>
                        {
                            { "accessToken", accessToken }, { "refreshToken", refreshToken },
                            { "expiresAt", UnixMilliseconds(expiry) }, { "subscriptionType", "max" },
                            { "scopes", new[] { "user:profile", "user:inference" } }
                        }
                    }
                }));
            }

            internal ClaudeQuotaResponse Success()
            {
                return new ClaudeQuotaResponse { Status = 200, Body = "{\"five_hour\":{\"utilization\":25,\"resets_at\":\"" +
                    Now.AddHours(2).ToString("o", CultureInfo.InvariantCulture) + "\"}}" };
            }

            public void Dispose()
            {
                foreach (string file in Directory.GetFiles(directory)) File.Delete(file);
                Directory.Delete(directory);
            }
        }

        private static Dictionary<string, object> Parse(string value)
        {
            return (Dictionary<string, object>)new JavaScriptSerializer().DeserializeObject(value);
        }

        private static Dictionary<string, object> ConvertedWindows(string input, DateTime now)
        {
            string converted = ClaudeQuotaClient.ConvertUsage(input, now);
            Assert(converted != null, "valid quota windows are converted");
            return (Dictionary<string, object>)Parse(converted)["rate_limits"];
        }

        private static long UnixMilliseconds(DateTime value)
        {
            return (long)(value - new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc)).TotalMilliseconds;
        }

        private static void Assert(bool condition, string message)
        {
            if (!condition) throw new InvalidOperationException(message);
        }
    }
}
