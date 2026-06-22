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
    internal sealed class AuthData
    {
        public string AccessToken;
        public string AccountId;

        public static string GetCodexHome()
        {
            string codexHome = Environment.GetEnvironmentVariable("CODEX_HOME");
            if (string.IsNullOrEmpty(codexHome))
            {
                codexHome = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".codex");
            }

            return codexHome;
        }

        public static AuthData Load()
        {
            string authPath = Path.Combine(GetCodexHome(), "auth.json");
            if (!File.Exists(authPath))
            {
                return null;
            }

            string json = File.ReadAllText(authPath);
            object rootObject = new JavaScriptSerializer().DeserializeObject(json);
            Dictionary<string, object> root = rootObject as Dictionary<string, object>;
            if (root == null)
            {
                return null;
            }

            object tokensObject;
            if (!root.TryGetValue("tokens", out tokensObject))
            {
                return null;
            }

            Dictionary<string, object> tokens = tokensObject as Dictionary<string, object>;
            if (tokens == null)
            {
                return null;
            }

            AuthData authData = new AuthData();
            authData.AccessToken = GetString(tokens, "access_token");
            authData.AccountId = GetString(tokens, "account_id");
            return authData;
        }

        private static string GetString(Dictionary<string, object> values, string key)
        {
            object value;
            if (values.TryGetValue(key, out value) && value != null)
            {
                return value.ToString();
            }

            return null;
        }
    }
}
