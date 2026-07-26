using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;

namespace CodexUsageTray
{
    internal static class CodexRateLimitClient
    {
        private const int FiveHourWindowMinutes = 300;
        private const int WeeklyWindowMinutes = 10080;
        private const int RequestTimeoutMilliseconds = 20000;
        private const int ShutdownTimeoutMilliseconds = 1500;
        private const int KillTimeoutMilliseconds = 1000;

        private enum FetchErrorKind
        {
            CommandNotFound,
            StartFailed,
            TimedOut,
            AuthenticationRequired,
            ExitedEarly,
            RequestRejected,
            LimitsUnavailable,
            InvalidResponse
        }

        private enum ResponseWaitResult
        {
            Success,
            TimedOut,
            EndOfStream
        }

        private delegate void AppServerRequestSender(
            StreamWriter input,
            JavaScriptSerializer serializer);

        public static UsageSnapshot FetchUsage()
        {
            DateTime attemptedAt = DateTime.Now;
            AppServerRequestResult request = ExecuteAppServerRequest(SendRateLimitRequest);
            if (!request.IsSuccess)
            {
                return FromClassifiedError(
                    request.ErrorKind,
                    attemptedAt);
            }

            return ParseRateLimitsResponse(request.Response, attemptedAt);
        }

        public static ResetCreditRedemptionResult ConsumeResetCredit(
            string creditId,
            string idempotencyKey)
        {
            if (string.IsNullOrWhiteSpace(creditId))
            {
                return ResetCreditRedemptionResult.FromError(
                    "The reset credit did not include an ID.");
            }
            if (string.IsNullOrWhiteSpace(idempotencyKey))
            {
                return ResetCreditRedemptionResult.FromError(
                    "The reset request did not include an idempotency key.");
            }

            AppServerRequestResult request = ExecuteAppServerRequest(
                delegate(StreamWriter input, JavaScriptSerializer serializer)
                {
                    SendResetCreditRequest(input, serializer, creditId, idempotencyKey);
                });
            if (!request.IsSuccess)
            {
                return ResetCreditRedemptionResult.FromError(
                    GetRedemptionFailureMessage(request.ErrorKind));
            }

            return ParseResetCreditRedemptionResponse(request.Response);
        }

        internal static UsageSnapshot ParseRateLimitsResponse(string json)
        {
            return ParseRateLimitsResponse(json, DateTime.Now);
        }

        internal static UsageSnapshot ParseRateLimitsResponse(string json, DateTime attemptedAt)
        {
            if (string.IsNullOrWhiteSpace(json))
            {
                return FromClassifiedError(FetchErrorKind.InvalidResponse, attemptedAt);
            }

            try
            {
                object rootObject = CreateSerializer().DeserializeObject(json);
                Dictionary<string, object> root = rootObject as Dictionary<string, object>;
                if (root == null)
                {
                    return FromClassifiedError(FetchErrorKind.InvalidResponse, attemptedAt);
                }

                return ParseRateLimitsResponse(root, attemptedAt);
            }
            catch (ArgumentException)
            {
                return FromClassifiedError(FetchErrorKind.InvalidResponse, attemptedAt);
            }
            catch (InvalidOperationException)
            {
                return FromClassifiedError(FetchErrorKind.InvalidResponse, attemptedAt);
            }
        }

        internal static ResetCreditRedemptionResult ParseResetCreditRedemptionResponse(string json)
        {
            if (string.IsNullOrWhiteSpace(json))
            {
                return ResetCreditRedemptionResult.FromError(
                    "Codex returned no reset result.");
            }

            try
            {
                object rootObject = CreateSerializer().DeserializeObject(json);
                Dictionary<string, object> root = rootObject as Dictionary<string, object>;
                return root != null
                    ? ParseResetCreditRedemptionResponse(root)
                    : ResetCreditRedemptionResult.FromError(
                        "Codex returned invalid reset data.");
            }
            catch (ArgumentException)
            {
                return ResetCreditRedemptionResult.FromError(
                    "Codex returned invalid reset data.");
            }
            catch (InvalidOperationException)
            {
                return ResetCreditRedemptionResult.FromError(
                    "Codex returned invalid reset data.");
            }
        }

        private static ResetCreditRedemptionResult ParseResetCreditRedemptionResponse(
            Dictionary<string, object> response)
        {
            Dictionary<string, object> error;
            if (TryGetDictionary(response, "error", out error))
            {
                string message = GetOptionalString(error, "message");
                if (IsAuthenticationMessage(message))
                {
                    return ResetCreditRedemptionResult.FromError(
                        "Codex is not signed in. Run codex login.");
                }

                string detail = CompactErrorDetail(message, 160);
                return ResetCreditRedemptionResult.FromError(
                    string.IsNullOrEmpty(detail)
                        ? "Codex rejected the reset request."
                        : "Codex rejected the reset request: " + detail);
            }

            Dictionary<string, object> result;
            if (!TryGetDictionary(response, "result", out result))
            {
                return ResetCreditRedemptionResult.FromError(
                    "Codex returned invalid reset data.");
            }

            string outcome = GetOptionalString(result, "outcome");
            if (string.Equals(outcome, "reset", StringComparison.OrdinalIgnoreCase))
            {
                return ResetCreditRedemptionResult.FromOutcome(
                    ResetCreditRedemptionOutcome.Reset);
            }
            if (string.Equals(outcome, "alreadyRedeemed", StringComparison.OrdinalIgnoreCase))
            {
                return ResetCreditRedemptionResult.FromOutcome(
                    ResetCreditRedemptionOutcome.AlreadyRedeemed);
            }
            if (string.Equals(outcome, "nothingToReset", StringComparison.OrdinalIgnoreCase))
            {
                return ResetCreditRedemptionResult.FromOutcome(
                    ResetCreditRedemptionOutcome.NothingToReset);
            }
            if (string.Equals(outcome, "noCredit", StringComparison.OrdinalIgnoreCase))
            {
                return ResetCreditRedemptionResult.FromOutcome(
                    ResetCreditRedemptionOutcome.NoCredit);
            }

            return ResetCreditRedemptionResult.FromError(
                "Codex returned an unrecognized reset result.");
        }

        private static UsageSnapshot ParseRateLimitsResponse(
            Dictionary<string, object> response,
            DateTime attemptedAt)
        {
            Dictionary<string, object> error;
            if (TryGetDictionary(response, "error", out error))
            {
                return FromRpcError(error, attemptedAt);
            }

            Dictionary<string, object> payload = response;
            object resultObject;
            if (response.TryGetValue("result", out resultObject))
            {
                payload = resultObject as Dictionary<string, object>;
                if (payload == null)
                {
                    return FromClassifiedError(FetchErrorKind.InvalidResponse, attemptedAt);
                }
            }

            Dictionary<string, object> rateLimitsById = null;
            object rateLimitsByIdObject;
            if (payload.TryGetValue("rateLimitsByLimitId", out rateLimitsByIdObject) &&
                rateLimitsByIdObject != null)
            {
                rateLimitsById = rateLimitsByIdObject as Dictionary<string, object>;
                if (rateLimitsById == null)
                {
                    return FromClassifiedError(FetchErrorKind.InvalidResponse, attemptedAt);
                }
            }

            Dictionary<string, object> codexLimits = null;
            object rateLimitsObject;
            if (payload.TryGetValue("rateLimits", out rateLimitsObject) && rateLimitsObject != null)
            {
                codexLimits = rateLimitsObject as Dictionary<string, object>;
                if (codexLimits == null)
                {
                    return FromClassifiedError(FetchErrorKind.InvalidResponse, attemptedAt);
                }
            }

            Dictionary<string, object> mappedCodexLimits = GetLimitById(rateLimitsById, "codex");
            if (codexLimits == null)
            {
                codexLimits = mappedCodexLimits;
            }

            UsageSnapshot snapshot = new UsageSnapshot();
            snapshot.LastUpdated = attemptedAt;
            snapshot.LastAttempted = attemptedAt;

            if (codexLimits != null)
            {
                ParseLimitWindows(codexLimits, attemptedAt, out snapshot.FiveHour, out snapshot.Weekly);
                snapshot.PlanType = GetOptionalString(codexLimits, "planType");
            }

            if (string.IsNullOrEmpty(snapshot.PlanType) && mappedCodexLimits != null)
            {
                snapshot.PlanType = GetOptionalString(mappedCodexLimits, "planType");
            }

            snapshot.AvailableResetCount = ParseAvailableResetCount(payload);
            snapshot.AvailableResets = ParseAvailableResetCredits(payload);
            snapshot.AdditionalLimits = ParseAdditionalLimits(rateLimitsById, attemptedAt);

            if (!snapshot.HasPrimaryLimit)
            {
                return FromClassifiedError(FetchErrorKind.LimitsUnavailable, attemptedAt);
            }

            return snapshot;
        }

        private static List<UsageLimitSet> ParseAdditionalLimits(
            Dictionary<string, object> rateLimitsById,
            DateTime attemptedAt)
        {
            List<UsageLimitSet> limits = new List<UsageLimitSet>();
            if (rateLimitsById == null)
            {
                return limits;
            }

            foreach (KeyValuePair<string, object> entry in rateLimitsById)
            {
                if (string.Equals(entry.Key, "codex", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                Dictionary<string, object> limitSnapshot = entry.Value as Dictionary<string, object>;
                if (limitSnapshot == null)
                {
                    continue;
                }

                UsageLimitSet limit = new UsageLimitSet();
                limit.LimitId = entry.Key;
                limit.DisplayName = GetOptionalString(limitSnapshot, "limitName");
                if (string.IsNullOrWhiteSpace(limit.DisplayName))
                {
                    limit.DisplayName = entry.Key;
                }

                ParseLimitWindows(limitSnapshot, attemptedAt, out limit.FiveHour, out limit.Weekly);
                limits.Add(limit);
            }

            limits.Sort(delegate(UsageLimitSet left, UsageLimitSet right)
            {
                int displayNameOrder = string.Compare(
                    left != null ? left.DisplayName : null,
                    right != null ? right.DisplayName : null,
                    StringComparison.OrdinalIgnoreCase);
                if (displayNameOrder != 0)
                {
                    return displayNameOrder;
                }

                return string.Compare(
                    left != null ? left.LimitId : null,
                    right != null ? right.LimitId : null,
                    StringComparison.OrdinalIgnoreCase);
            });

            return limits;
        }

        private static void ParseLimitWindows(
            Dictionary<string, object> limitSnapshot,
            DateTime attemptedAt,
            out LimitWindow fiveHour,
            out LimitWindow weekly)
        {
            fiveHour = null;
            weekly = null;

            LimitWindow primary = ParseWindow(limitSnapshot, "primary", attemptedAt);
            AssignWindow(primary, ref fiveHour, ref weekly);

            LimitWindow secondary = ParseWindow(limitSnapshot, "secondary", attemptedAt);
            AssignWindow(secondary, ref fiveHour, ref weekly);
        }

        private static void AssignWindow(
            LimitWindow window,
            ref LimitWindow fiveHour,
            ref LimitWindow weekly)
        {
            if (window == null || !window.WindowMinutes.HasValue)
            {
                return;
            }

            if (window.WindowMinutes.Value == FiveHourWindowMinutes && fiveHour == null)
            {
                fiveHour = window;
            }
            else if (window.WindowMinutes.Value == WeeklyWindowMinutes && weekly == null)
            {
                weekly = window;
            }
        }

        private static LimitWindow ParseWindow(
            Dictionary<string, object> limitSnapshot,
            string name,
            DateTime attemptedAt)
        {
            Dictionary<string, object> windowValues;
            if (!TryGetDictionary(limitSnapshot, name, out windowValues))
            {
                return null;
            }

            object durationObject;
            long duration;
            if (!windowValues.TryGetValue("windowDurationMins", out durationObject) ||
                !TryGetInteger(durationObject, out duration) ||
                (duration != FiveHourWindowMinutes && duration != WeeklyWindowMinutes))
            {
                return null;
            }

            object usedPercentObject;
            double usedPercent;
            if (!windowValues.TryGetValue("usedPercent", out usedPercentObject) ||
                !TryGetFiniteDouble(usedPercentObject, out usedPercent) ||
                usedPercent < 0.0 || usedPercent > 100.0)
            {
                return null;
            }

            LimitWindow window = new LimitWindow();
            window.UsedPercent = usedPercent;
            window.WindowMinutes = (int)duration;

            object resetsAtObject;
            long resetsAt;
            if (windowValues.TryGetValue("resetsAt", out resetsAtObject) &&
                resetsAtObject != null &&
                TryGetInteger(resetsAtObject, out resetsAt))
            {
                window.ResetAfterSeconds = GetResetAfterSeconds(resetsAt, attemptedAt);
            }

            return window;
        }

        private static int? GetResetAfterSeconds(long resetsAtUnixSeconds, DateTime attemptedAt)
        {
            try
            {
                DateTime unixEpoch = new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc);
                DateTime resetsAt = unixEpoch.AddSeconds(resetsAtUnixSeconds);
                double remainingSeconds = (resetsAt - attemptedAt.ToUniversalTime()).TotalSeconds;
                if (double.IsNaN(remainingSeconds) || double.IsInfinity(remainingSeconds))
                {
                    return null;
                }
                if (remainingSeconds <= 0.0)
                {
                    return 0;
                }
                if (remainingSeconds >= int.MaxValue)
                {
                    return int.MaxValue;
                }

                return (int)Math.Ceiling(remainingSeconds);
            }
            catch (ArgumentOutOfRangeException)
            {
                return null;
            }
        }

        private static int? ParseAvailableResetCount(Dictionary<string, object> payload)
        {
            Dictionary<string, object> resetCredits;
            if (!TryGetDictionary(payload, "rateLimitResetCredits", out resetCredits))
            {
                return null;
            }

            object availableCountObject;
            long availableCount;
            if (!resetCredits.TryGetValue("availableCount", out availableCountObject) ||
                !TryGetInteger(availableCountObject, out availableCount) ||
                availableCount < 0 || availableCount > int.MaxValue)
            {
                return null;
            }

            return (int)availableCount;
        }

        private static List<RateLimitResetCredit> ParseAvailableResetCredits(
            Dictionary<string, object> payload)
        {
            List<RateLimitResetCredit> availableCredits = new List<RateLimitResetCredit>();
            Dictionary<string, object> resetCredits;
            if (!TryGetDictionary(payload, "rateLimitResetCredits", out resetCredits))
            {
                return availableCredits;
            }

            object creditsObject;
            object[] credits;
            if (!resetCredits.TryGetValue("credits", out creditsObject) ||
                creditsObject == null ||
                (credits = creditsObject as object[]) == null)
            {
                return availableCredits;
            }

            for (int index = 0; index < credits.Length; index++)
            {
                Dictionary<string, object> values = credits[index] as Dictionary<string, object>;
                if (values == null)
                {
                    continue;
                }

                string status = GetOptionalString(values, "status");
                if (!string.IsNullOrEmpty(status) &&
                    !string.Equals(status, "available", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                RateLimitResetCredit credit = new RateLimitResetCredit();
                credit.Id = GetOptionalString(values, "id");
                credit.ResetType = GetOptionalString(values, "resetType");
                credit.Title = GetOptionalString(values, "title");

                object expiresAtObject;
                long expiresAt;
                if (values.TryGetValue("expiresAt", out expiresAtObject) &&
                    expiresAtObject != null &&
                    TryGetInteger(expiresAtObject, out expiresAt))
                {
                    credit.ExpiresAtUtc = GetUnixDateTimeUtc(expiresAt);
                }

                availableCredits.Add(credit);
            }

            availableCredits.Sort(delegate(
                RateLimitResetCredit left,
                RateLimitResetCredit right)
            {
                DateTime leftExpiration = left != null && left.ExpiresAtUtc.HasValue
                    ? left.ExpiresAtUtc.Value
                    : DateTime.MaxValue;
                DateTime rightExpiration = right != null && right.ExpiresAtUtc.HasValue
                    ? right.ExpiresAtUtc.Value
                    : DateTime.MaxValue;
                return leftExpiration.CompareTo(rightExpiration);
            });

            return availableCredits;
        }

        private static DateTime? GetUnixDateTimeUtc(long unixSeconds)
        {
            try
            {
                return new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc)
                    .AddSeconds(unixSeconds);
            }
            catch (ArgumentOutOfRangeException)
            {
                return null;
            }
        }

        private static Dictionary<string, object> GetLimitById(
            Dictionary<string, object> rateLimitsById,
            string limitId)
        {
            if (rateLimitsById == null)
            {
                return null;
            }

            foreach (KeyValuePair<string, object> entry in rateLimitsById)
            {
                if (string.Equals(entry.Key, limitId, StringComparison.OrdinalIgnoreCase))
                {
                    return entry.Value as Dictionary<string, object>;
                }
            }

            return null;
        }

        private static ResponseWaitResult WaitForResponse(
            Process process,
            ProcessOutputBuffer output,
            JavaScriptSerializer serializer,
            Stopwatch timer,
            int requestId,
            out Dictionary<string, object> response)
        {
            response = null;

            while (timer.ElapsedMilliseconds < RequestTimeoutMilliseconds)
            {
                string line;
                while (output.TryDequeue(out line))
                {
                    Dictionary<string, object> candidate;
                    if (TryDeserializeObject(serializer, line, out candidate) &&
                        HasRequestId(candidate, requestId))
                    {
                        response = candidate;
                        return ResponseWaitResult.Success;
                    }
                }

                if (output.EndOfStream)
                {
                    return ResponseWaitResult.EndOfStream;
                }

                long remaining = RequestTimeoutMilliseconds - timer.ElapsedMilliseconds;
                int waitMilliseconds = (int)Math.Min(100L, Math.Max(1L, remaining));
                output.Wait(waitMilliseconds);
            }

            return ResponseWaitResult.TimedOut;
        }

        private static bool TryDeserializeObject(
            JavaScriptSerializer serializer,
            string json,
            out Dictionary<string, object> value)
        {
            value = null;
            if (string.IsNullOrWhiteSpace(json))
            {
                return false;
            }

            try
            {
                value = serializer.DeserializeObject(json) as Dictionary<string, object>;
                return value != null;
            }
            catch (ArgumentException)
            {
                return false;
            }
            catch (InvalidOperationException)
            {
                return false;
            }
        }

        private static bool HasRequestId(Dictionary<string, object> response, int requestId)
        {
            object idObject;
            if (!response.TryGetValue("id", out idObject) || idObject == null)
            {
                return false;
            }

            string stringId = idObject as string;
            if (stringId != null)
            {
                return string.Equals(
                    stringId,
                    requestId.ToString(CultureInfo.InvariantCulture),
                    StringComparison.Ordinal);
            }

            long numericId;
            return TryGetInteger(idObject, out numericId) && numericId == requestId;
        }

        private static void SendInitialize(StreamWriter input, JavaScriptSerializer serializer)
        {
            Dictionary<string, object> clientInfo = new Dictionary<string, object>();
            clientInfo["name"] = "codex-usage-tray";
            clientInfo["title"] = "Codex Usage Tray";
            clientInfo["version"] = "1.0";

            Dictionary<string, object> parameters = new Dictionary<string, object>();
            parameters["clientInfo"] = clientInfo;
            parameters["capabilities"] = new Dictionary<string, object>();

            Dictionary<string, object> request = new Dictionary<string, object>();
            request["id"] = 1;
            request["method"] = "initialize";
            request["params"] = parameters;
            SendMessage(input, serializer, request);
        }

        private static void SendInitialized(StreamWriter input, JavaScriptSerializer serializer)
        {
            Dictionary<string, object> notification = new Dictionary<string, object>();
            notification["method"] = "initialized";
            SendMessage(input, serializer, notification);
        }

        private static void SendRateLimitRequest(StreamWriter input, JavaScriptSerializer serializer)
        {
            Dictionary<string, object> request = new Dictionary<string, object>();
            request["id"] = 2;
            request["method"] = "account/rateLimits/read";
            SendMessage(input, serializer, request);
        }

        private static void SendResetCreditRequest(
            StreamWriter input,
            JavaScriptSerializer serializer,
            string creditId,
            string idempotencyKey)
        {
            Dictionary<string, object> parameters = new Dictionary<string, object>();
            parameters["idempotencyKey"] = idempotencyKey;
            parameters["creditId"] = creditId;

            Dictionary<string, object> request = new Dictionary<string, object>();
            request["id"] = 2;
            request["method"] = "account/rateLimitResetCredit/consume";
            request["params"] = parameters;
            SendMessage(input, serializer, request);
        }

        private static void SendMessage(
            StreamWriter input,
            JavaScriptSerializer serializer,
            Dictionary<string, object> message)
        {
            input.WriteLine(serializer.Serialize(message));
            input.Flush();
        }

        private static JavaScriptSerializer CreateSerializer()
        {
            JavaScriptSerializer serializer = new JavaScriptSerializer();
            serializer.MaxJsonLength = 4 * 1024 * 1024;
            return serializer;
        }

        private static string ResolveCodexCommand()
        {
            string appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
            if (!string.IsNullOrEmpty(appData))
            {
                string nativeCommand = FindBundledCodexExecutable(appData);
                if (!string.IsNullOrEmpty(nativeCommand))
                {
                    return nativeCommand;
                }

                string npmCommand = Path.Combine(appData, "npm", "codex.cmd");
                string npmScript = Path.Combine(
                    appData,
                    "npm",
                    "node_modules",
                    "@openai",
                    "codex",
                    "bin",
                    "codex.js");
                if (File.Exists(npmCommand) && File.Exists(npmScript))
                {
                    return Path.GetFullPath(npmCommand);
                }
            }

            string pathValue = Environment.GetEnvironmentVariable("PATH");
            if (string.IsNullOrWhiteSpace(pathValue))
            {
                return null;
            }

            string[] extensions = { ".exe", ".com", ".cmd", ".bat", "" };
            string[] pathEntries = pathValue.Split(Path.PathSeparator);
            for (int pathIndex = 0; pathIndex < pathEntries.Length; pathIndex++)
            {
                string directory = pathEntries[pathIndex].Trim().Trim('"');
                if (string.IsNullOrWhiteSpace(directory))
                {
                    continue;
                }

                directory = Environment.ExpandEnvironmentVariables(directory);
                for (int extensionIndex = 0; extensionIndex < extensions.Length; extensionIndex++)
                {
                    try
                    {
                        string candidate = Path.Combine(directory, "codex" + extensions[extensionIndex]);
                        if (File.Exists(candidate))
                        {
                            return Path.GetFullPath(candidate);
                        }
                    }
                    catch (Exception)
                    {
                        // Ignore malformed PATH entries and continue searching.
                    }
                }
            }

            return null;
        }

        private static string FindBundledCodexExecutable(string appData)
        {
            string packageRoot = Path.Combine(
                appData,
                "npm",
                "node_modules",
                "@openai",
                "codex",
                "node_modules",
                "@openai");
            if (!Directory.Exists(packageRoot))
            {
                return null;
            }

            try
            {
                string[] packages = Directory.GetDirectories(packageRoot, "codex-win32-*");
                for (int packageIndex = 0; packageIndex < packages.Length; packageIndex++)
                {
                    string[] executables = Directory.GetFiles(
                        packages[packageIndex],
                        "codex.exe",
                        SearchOption.AllDirectories);
                    if (executables.Length > 0)
                    {
                        return Path.GetFullPath(executables[0]);
                    }
                }
            }
            catch (Exception)
            {
                return null;
            }

            return null;
        }

        private static ProcessStartInfo CreateStartInfo(string codexCommand)
        {
            ProcessStartInfo startInfo = new ProcessStartInfo();
            string extension = Path.GetExtension(codexCommand);
            if (string.Equals(extension, ".cmd", StringComparison.OrdinalIgnoreCase) ||
                string.Equals(extension, ".bat", StringComparison.OrdinalIgnoreCase))
            {
                startInfo.FileName = ResolveCommandInterpreter();
                startInfo.Arguments = "/d /s /c \"\"" + codexCommand + "\" app-server --stdio\"";
            }
            else
            {
                startInfo.FileName = codexCommand;
                startInfo.Arguments = "app-server --stdio";
            }

            startInfo.UseShellExecute = false;
            startInfo.CreateNoWindow = true;
            startInfo.WindowStyle = ProcessWindowStyle.Hidden;
            startInfo.RedirectStandardInput = true;
            startInfo.RedirectStandardOutput = true;
            startInfo.RedirectStandardError = true;
            startInfo.StandardOutputEncoding = Encoding.UTF8;
            startInfo.StandardErrorEncoding = Encoding.UTF8;
            return startInfo;
        }

        private static string ResolveCommandInterpreter()
        {
            string commandInterpreter = Environment.GetEnvironmentVariable("ComSpec");
            if (!string.IsNullOrWhiteSpace(commandInterpreter) && File.Exists(commandInterpreter))
            {
                return commandInterpreter;
            }

            return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "cmd.exe");
        }

        private static FetchErrorKind GetWaitErrorKind(
            ResponseWaitResult waitResult,
            ProcessErrorHints errorHints)
        {
            if (errorHints != null && errorHints.AuthenticationRequired)
            {
                return FetchErrorKind.AuthenticationRequired;
            }

            return waitResult == ResponseWaitResult.TimedOut
                ? FetchErrorKind.TimedOut
                : FetchErrorKind.ExitedEarly;
        }

        private static UsageSnapshot FromRpcError(
            Dictionary<string, object> error,
            DateTime attemptedAt)
        {
            string message = GetOptionalString(error, "message");
            FetchErrorKind kind = IsAuthenticationMessage(message)
                ? FetchErrorKind.AuthenticationRequired
                : FetchErrorKind.RequestRejected;
            return FromClassifiedError(kind, attemptedAt, CompactErrorDetail(message, 160));
        }

        private static bool IsAuthenticationMessage(string message)
        {
            if (string.IsNullOrEmpty(message))
            {
                return false;
            }

            string lower = message.ToLowerInvariant();
            return lower.Contains("not logged in") ||
                lower.Contains("not signed in") ||
                lower.Contains("unauthorized") ||
                lower.Contains("authentication") ||
                lower.Contains("codex login") ||
                lower.Contains("401");
        }

        private static UsageSnapshot FromClassifiedError(FetchErrorKind kind, DateTime attemptedAt)
        {
            return FromClassifiedError(kind, attemptedAt, null);
        }

        private static UsageSnapshot FromClassifiedError(
            FetchErrorKind kind,
            DateTime attemptedAt,
            string detail)
        {
            string message;
            switch (kind)
            {
                case FetchErrorKind.CommandNotFound:
                    message = "Codex CLI was not found. Install Codex or add it to PATH.";
                    break;
                case FetchErrorKind.StartFailed:
                    message = "Codex app-server could not be started.";
                    break;
                case FetchErrorKind.TimedOut:
                    message = "Codex app-server timed out while reading usage.";
                    break;
                case FetchErrorKind.AuthenticationRequired:
                    message = "Codex is not signed in. Run codex login.";
                    break;
                case FetchErrorKind.ExitedEarly:
                    message = "Codex app-server exited before returning usage.";
                    break;
                case FetchErrorKind.RequestRejected:
                    message = string.IsNullOrEmpty(detail)
                        ? "Codex app-server could not read usage limits."
                        : "Codex app-server rejected the usage request: " + detail;
                    break;
                case FetchErrorKind.LimitsUnavailable:
                    message = UsageSnapshot.PrimaryLimitsUnavailableMessage;
                    break;
                default:
                    message = "Codex app-server returned invalid usage data.";
                    break;
            }

            UsageSnapshot snapshot = UsageSnapshot.FromError(message);
            snapshot.LastAttempted = attemptedAt;
            return snapshot;
        }

        private static string GetRedemptionFailureMessage(FetchErrorKind kind)
        {
            switch (kind)
            {
                case FetchErrorKind.CommandNotFound:
                    return "Codex CLI was not found. Install Codex or add it to PATH.";
                case FetchErrorKind.StartFailed:
                    return "Codex app-server could not be started.";
                case FetchErrorKind.TimedOut:
                    return "Codex app-server timed out while using the reset credit.";
                case FetchErrorKind.AuthenticationRequired:
                    return "Codex is not signed in. Run codex login.";
                case FetchErrorKind.ExitedEarly:
                    return "Codex app-server exited before returning the reset result.";
                case FetchErrorKind.RequestRejected:
                    return "Codex app-server rejected the reset request.";
                default:
                    return "Codex app-server returned invalid reset data.";
            }
        }

        private static string CompactErrorDetail(string value, int maxLength)
        {
            if (string.IsNullOrWhiteSpace(value) || maxLength <= 0)
            {
                return null;
            }

            StringBuilder compact = new StringBuilder(Math.Min(value.Length, maxLength));
            bool pendingSpace = false;
            for (int index = 0; index < value.Length && compact.Length < maxLength; index++)
            {
                char current = value[index];
                if (char.IsControl(current) || char.IsWhiteSpace(current))
                {
                    pendingSpace = compact.Length > 0;
                    continue;
                }

                if (pendingSpace && compact.Length < maxLength)
                {
                    if (compact.Length >= maxLength - 1)
                    {
                        break;
                    }
                    compact.Append(' ');
                }
                pendingSpace = false;
                if (compact.Length < maxLength)
                {
                    compact.Append(current);
                }
            }

            return compact.Length == 0 ? null : compact.ToString();
        }

        private static bool TryGetDictionary(
            Dictionary<string, object> values,
            string name,
            out Dictionary<string, object> result)
        {
            result = null;
            if (values == null)
            {
                return false;
            }

            object value;
            if (!values.TryGetValue(name, out value) || value == null)
            {
                return false;
            }

            result = value as Dictionary<string, object>;
            return result != null;
        }

        private static string GetOptionalString(Dictionary<string, object> values, string name)
        {
            if (values == null)
            {
                return null;
            }

            object value;
            string text;
            if (!values.TryGetValue(name, out value) ||
                value == null ||
                (text = value as string) == null)
            {
                return null;
            }

            text = text.Trim();
            return text.Length == 0 ? null : text;
        }

        private static bool TryGetInteger(object value, out long result)
        {
            result = 0;
            if (value == null || value is bool || value is char || value is string)
            {
                return false;
            }

            if (value is float || value is double)
            {
                double numeric = Convert.ToDouble(value, CultureInfo.InvariantCulture);
                if (double.IsNaN(numeric) ||
                    double.IsInfinity(numeric) ||
                    numeric < long.MinValue ||
                    numeric > long.MaxValue ||
                    Math.Truncate(numeric) != numeric)
                {
                    return false;
                }

                result = (long)numeric;
                return true;
            }

            if (!(value is byte) && !(value is sbyte) &&
                !(value is short) && !(value is ushort) &&
                !(value is int) && !(value is uint) &&
                !(value is long) && !(value is ulong) &&
                !(value is decimal))
            {
                return false;
            }

            try
            {
                decimal numeric = Convert.ToDecimal(value, CultureInfo.InvariantCulture);
                if (decimal.Truncate(numeric) != numeric ||
                    numeric < long.MinValue || numeric > long.MaxValue)
                {
                    return false;
                }

                result = decimal.ToInt64(numeric);
                return true;
            }
            catch (OverflowException)
            {
                return false;
            }
        }

        private static bool TryGetFiniteDouble(object value, out double result)
        {
            result = 0.0;
            if (value == null || value is bool || value is char || value is string)
            {
                return false;
            }

            if (!(value is byte) && !(value is sbyte) &&
                !(value is short) && !(value is ushort) &&
                !(value is int) && !(value is uint) &&
                !(value is long) && !(value is ulong) &&
                !(value is float) && !(value is double) &&
                !(value is decimal))
            {
                return false;
            }

            try
            {
                result = Convert.ToDouble(value, CultureInfo.InvariantCulture);
                return !double.IsNaN(result) && !double.IsInfinity(result);
            }
            catch (OverflowException)
            {
                result = 0.0;
                return false;
            }
        }

        private static void CloseStandardInput(Process process)
        {
            if (process == null)
            {
                return;
            }

            try
            {
                process.StandardInput.Close();
            }
            catch (Exception)
            {
                // The process may already have closed its redirected input.
            }
        }

        private static void StopProcess(Process process, bool processStarted)
        {
            if (process == null || !processStarted)
            {
                return;
            }

            try
            {
                if (process.WaitForExit(ShutdownTimeoutMilliseconds))
                {
                    return;
                }
            }
            catch (Exception)
            {
                // Fall through to the guarded kill attempt.
            }

            try
            {
                if (!process.HasExited)
                {
                    process.Kill();
                }
            }
            catch (Exception)
            {
                return;
            }

            try
            {
                process.WaitForExit(KillTimeoutMilliseconds);
            }
            catch (Exception)
            {
                // Cleanup is best-effort after killing only the process we started.
            }
        }

        private static AppServerRequestResult ExecuteAppServerRequest(
            AppServerRequestSender sendRequest)
        {
            string codexCommand = ResolveCodexCommand();
            if (string.IsNullOrEmpty(codexCommand))
            {
                return AppServerRequestResult.FromError(FetchErrorKind.CommandNotFound);
            }

            Process process = null;
            ProcessOutputBuffer output = null;
            ProcessErrorHints errorHints = null;
            bool processStarted = false;
            bool requestSent = false;

            try
            {
                JavaScriptSerializer serializer = CreateSerializer();
                process = new Process();
                process.StartInfo = CreateStartInfo(codexCommand);
                output = new ProcessOutputBuffer();
                errorHints = new ProcessErrorHints();
                process.OutputDataReceived += output.HandleDataReceived;
                process.ErrorDataReceived += errorHints.HandleDataReceived;

                if (!process.Start())
                {
                    return AppServerRequestResult.FromError(FetchErrorKind.StartFailed);
                }

                processStarted = true;
                process.BeginOutputReadLine();
                process.BeginErrorReadLine();

                Stopwatch timer = Stopwatch.StartNew();
                SendInitialize(process.StandardInput, serializer);

                Dictionary<string, object> initializeResponse;
                ResponseWaitResult initializeWait = WaitForResponse(
                    process,
                    output,
                    serializer,
                    timer,
                    1,
                    out initializeResponse);
                if (initializeWait != ResponseWaitResult.Success)
                {
                    return AppServerRequestResult.FromError(
                        GetWaitErrorKind(initializeWait, errorHints));
                }

                Dictionary<string, object> initializeError;
                if (TryGetDictionary(initializeResponse, "error", out initializeError))
                {
                    return AppServerRequestResult.FromResponse(initializeResponse);
                }

                Dictionary<string, object> initializeResult;
                if (!TryGetDictionary(initializeResponse, "result", out initializeResult))
                {
                    return AppServerRequestResult.FromError(FetchErrorKind.InvalidResponse);
                }

                SendInitialized(process.StandardInput, serializer);
                sendRequest(process.StandardInput, serializer);
                requestSent = true;

                Dictionary<string, object> response;
                ResponseWaitResult requestWait = WaitForResponse(
                    process,
                    output,
                    serializer,
                    timer,
                    2,
                    out response);
                if (requestWait != ResponseWaitResult.Success)
                {
                    return AppServerRequestResult.FromError(
                        GetWaitErrorKind(requestWait, errorHints));
                }

                CloseStandardInput(process);
                return AppServerRequestResult.FromResponse(response);
            }
            catch (Exception)
            {
                if (errorHints != null && errorHints.AuthenticationRequired)
                {
                    return AppServerRequestResult.FromError(
                        FetchErrorKind.AuthenticationRequired);
                }

                return AppServerRequestResult.FromError(
                    requestSent ? FetchErrorKind.ExitedEarly : FetchErrorKind.StartFailed);
            }
            finally
            {
                CloseStandardInput(process);
                StopProcess(process, processStarted);

                if (process != null && output != null)
                {
                    process.OutputDataReceived -= output.HandleDataReceived;
                }
                if (process != null && errorHints != null)
                {
                    process.ErrorDataReceived -= errorHints.HandleDataReceived;
                }
                if (output != null)
                {
                    output.Dispose();
                }
                if (process != null)
                {
                    process.Dispose();
                }
            }
        }

        private sealed class AppServerRequestResult
        {
            public Dictionary<string, object> Response;
            public FetchErrorKind ErrorKind;

            public bool IsSuccess
            {
                get { return Response != null; }
            }

            public static AppServerRequestResult FromResponse(
                Dictionary<string, object> response)
            {
                AppServerRequestResult result = new AppServerRequestResult();
                result.Response = response;
                return result;
            }

            public static AppServerRequestResult FromError(FetchErrorKind kind)
            {
                AppServerRequestResult result = new AppServerRequestResult();
                result.ErrorKind = kind;
                return result;
            }
        }

        private sealed class ProcessOutputBuffer : IDisposable
        {
            private readonly object syncRoot = new object();
            private readonly Queue<string> lines = new Queue<string>();
            private readonly AutoResetEvent dataAvailable = new AutoResetEvent(false);
            private bool endOfStream;

            public bool EndOfStream
            {
                get
                {
                    lock (syncRoot)
                    {
                        return endOfStream && lines.Count == 0;
                    }
                }
            }

            public void HandleDataReceived(object sender, DataReceivedEventArgs args)
            {
                lock (syncRoot)
                {
                    if (args.Data == null)
                    {
                        endOfStream = true;
                    }
                    else
                    {
                        lines.Enqueue(args.Data);
                    }
                }

                try
                {
                    dataAvailable.Set();
                }
                catch (ObjectDisposedException)
                {
                    // Process shutdown raced with disposal.
                }
            }

            public bool TryDequeue(out string line)
            {
                lock (syncRoot)
                {
                    if (lines.Count == 0)
                    {
                        line = null;
                        return false;
                    }

                    line = lines.Dequeue();
                    return true;
                }
            }

            public void Wait(int milliseconds)
            {
                dataAvailable.WaitOne(milliseconds);
            }

            public void Dispose()
            {
                dataAvailable.Dispose();
            }
        }

        private sealed class ProcessErrorHints
        {
            private volatile bool authenticationRequired;

            public bool AuthenticationRequired
            {
                get { return authenticationRequired; }
            }

            public void HandleDataReceived(object sender, DataReceivedEventArgs args)
            {
                if (args.Data != null && IsAuthenticationMessage(args.Data))
                {
                    authenticationRequired = true;
                }
            }
        }
    }
}
