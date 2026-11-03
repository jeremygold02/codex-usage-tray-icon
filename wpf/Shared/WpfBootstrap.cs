using System.Windows;
using Wpf.Ui.Appearance;
using Wpf.Ui.Markup;

namespace CodexUsageTray
{
    internal static class WpfBootstrap
    {
        public static void Initialize()
        {
            if (Application.Current != null) return;

            Application application = new Application();
            application.ShutdownMode = ShutdownMode.OnExplicitShutdown;
            application.Resources.MergedDictionaries.Add(new ThemesDictionary { Theme = ApplicationTheme.Dark });
            application.Resources.MergedDictionaries.Add(new ControlsDictionary());
        }
    }
}
