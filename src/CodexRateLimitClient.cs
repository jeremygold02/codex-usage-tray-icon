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
    internal static class CodexRateLimitClient
    {
        private const string Endpoint = "https://chatgpt.com/backend-api/codex/responses";

        public static UsageSnapshot FetchUsage()
        {
            AuthData authData = AuthData.Load();
            if (authData == null || string.IsNullOrEmpty(authData.AccessToken))
            {
                return UsageSnapshot.FromError("Codex auth not found; run codex login");
            }

            string sessionId = GenerateSessionId();
            byte[] payloadBytes = Encoding.UTF8.GetBytes(BuildPayload(sessionId, LoadConfiguredModel()));

            HttpWebRequest request = (HttpWebRequest)WebRequest.Create(Endpoint);
            request.Method = "POST";
            request.AllowAutoRedirect = false;
            request.Timeout = 20000;
            request.ReadWriteTimeout = 20000;
            request.Accept = "text/event-stream";
            request.ContentType = "application/json";
            request.UserAgent = "codex-usage-tray/1.0.0";
            request.Headers.Add("OpenAI-Beta", "responses=experimental");
            request.Headers.Add("session_id", sessionId);
            request.Headers.Add("originator", "codex_usage_tray");
            request.Headers.Add("Authorization", "Bearer " + authData.AccessToken);
            request.Headers.Add("Cache-Control", "no-cache");
            if (!string.IsNullOrEmpty(authData.AccountId))
            {
                request.Headers.Add("chatgpt-account-id", authData.AccountId);
            }

            request.ContentLength = payloadBytes.Length;
            using (Stream requestStream = request.GetRequestStream())
            {
                requestStream.Write(payloadBytes, 0, payloadBytes.Length);
            }

            try
            {
                using (HttpWebResponse response = (HttpWebResponse)request.GetResponse())
                {
                    return ParseSnapshot(response.Headers, null);
                }
            }
            catch (WebException ex)
            {
                HttpWebResponse response = ex.Response as HttpWebResponse;
                if (response != null)
                {
                    using (response)
                    {
                        UsageSnapshot snapshot = ParseSnapshot(response.Headers, null);
                        if (snapshot.HasAnyLimit)
                        {
                            return snapshot;
                        }

                        string errorBody = ReadResponseBody(response);
                        string errorMessage = "Codex request failed: HTTP " + (int)response.StatusCode;
                        if (!string.IsNullOrEmpty(errorBody))
                        {
                            errorMessage += " - " + errorBody;
                        }
                        return UsageSnapshot.FromError(errorMessage);
                    }
                }

                return UsageSnapshot.FromError("Codex request failed: " + ex.Message);
            }
        }

        private static UsageSnapshot ParseSnapshot(NameValueCollection headers, string error)
        {
            UsageSnapshot snapshot = new UsageSnapshot();
            snapshot.LastUpdated = DateTime.Now;
            snapshot.ErrorMessage = error;
            snapshot.FiveHour = ParseWindow(headers, "x-codex-primary");
            snapshot.Weekly = ParseWindow(headers, "x-codex-secondary");
            return snapshot;
        }

        private static LimitWindow ParseWindow(NameValueCollection headers, string prefix)
        {
            double usedPercent;
            if (!TryGetDoubleHeader(headers, prefix + "-used-percent", out usedPercent))
            {
                return null;
            }

            LimitWindow window = new LimitWindow();
            window.UsedPercent = usedPercent;
            window.WindowMinutes = TryGetIntHeader(headers, prefix + "-window-minutes");
            window.ResetAfterSeconds = TryGetIntHeader(headers, prefix + "-reset-after-seconds");
            return window;
        }

        private static bool TryGetDoubleHeader(NameValueCollection headers, string name, out double value)
        {
            string raw = GetHeader(headers, name);
            if (raw != null && double.TryParse(raw, System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out value))
            {
                return true;
            }

            value = 0;
            return false;
        }

        private static int? TryGetIntHeader(NameValueCollection headers, string name)
        {
            string raw = GetHeader(headers, name);
            int value;
            if (raw != null && int.TryParse(raw, out value))
            {
                return value;
            }

            return null;
        }

        private static string GetHeader(NameValueCollection headers, string name)
        {
            if (headers == null)
            {
                return null;
            }

            foreach (string key in headers.AllKeys)
            {
                if (string.Equals(key, name, StringComparison.OrdinalIgnoreCase))
                {
                    return headers[key];
                }
            }

            return null;
        }

        private static string LoadConfiguredModel()
        {
            string configPath = Path.Combine(AuthData.GetCodexHome(), "config.toml");
            if (File.Exists(configPath))
            {
                foreach (string rawLine in File.ReadAllLines(configPath))
                {
                    string line = rawLine.Trim();
                    if (!line.StartsWith("model ", StringComparison.OrdinalIgnoreCase) &&
                        !line.StartsWith("model=", StringComparison.OrdinalIgnoreCase))
                    {
                        continue;
                    }

                    int separator = line.IndexOf('=');
                    if (separator < 0)
                    {
                        continue;
                    }

                    string model = line.Substring(separator + 1).Trim().Trim('"', '\'');
                    if (!string.IsNullOrEmpty(model))
                    {
                        return model;
                    }
                }
            }

            return "gpt-5.5";
        }

        private static string ReadResponseBody(HttpWebResponse response)
        {
            try
            {
                using (Stream stream = response.GetResponseStream())
                {
                    if (stream == null)
                    {
                        return null;
                    }

                    using (StreamReader reader = new StreamReader(stream))
                    {
                        string body = reader.ReadToEnd();
                        if (body.Length > 240)
                        {
                            return body.Substring(0, 240);
                        }
                        return body;
                    }
                }
            }
            catch
            {
                return null;
            }
        }

        private static string BuildPayload(string sessionId, string model)
        {
            Dictionary<string, object> text = new Dictionary<string, object>();
            text["type"] = "input_text";
            text["text"] = "hi";

            Dictionary<string, object> message = new Dictionary<string, object>();
            message["type"] = "message";
            message["id"] = null;
            message["role"] = "user";
            message["content"] = new object[] { text };

            Dictionary<string, object> reasoning = new Dictionary<string, object>();
            reasoning["effort"] = "medium";
            reasoning["summary"] = "auto";

            Dictionary<string, object> payload = new Dictionary<string, object>();
            payload["model"] = model;
            payload["instructions"] = "You are a coding agent running in the Codex CLI, a terminal-based coding assistant.";
            payload["input"] = new object[] { message };
            payload["tools"] = new object[0];
            payload["tool_choice"] = "auto";
            payload["parallel_tool_calls"] = false;
            payload["reasoning"] = reasoning;
            payload["store"] = false;
            payload["stream"] = true;
            payload["include"] = new object[] { "reasoning.encrypted_content" };
            payload["prompt_cache_key"] = sessionId;

            return new JavaScriptSerializer().Serialize(payload);
        }

        private static string GenerateSessionId()
        {
            byte[] bytes = new byte[16];
            new Random().NextBytes(bytes);
            StringBuilder builder = new StringBuilder(bytes.Length * 2);
            for (int i = 0; i < bytes.Length; i++)
            {
                builder.Append(bytes[i].ToString("x2"));
            }
            return builder.ToString();
        }
    }
}
