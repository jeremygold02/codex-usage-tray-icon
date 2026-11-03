using System;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using Wpf.Ui.Controls;

namespace CodexUsageTray
{
    public partial class WpfSettingsView : UserControl
    {
        private readonly SettingsForm owner;
        internal bool Synchronizing;
        private bool? darkTheme;

        internal WpfSettingsView(SettingsForm owner)
        {
            this.owner = owner;
            InitializeComponent();
            ToggleButton[] switches = { MetricWeekly, MetricFiveHour, MetricAuto, ThresholdAlerts, ReminderEnabled,
                AutoUseEnabled, ForecastLearned, ForecastRecent, ShowProjection, HighlightQuiet, StartWithWindows,
                TrayBackground, ShowUpdatedTime, ShowResetTimes, ColorUsageBars };
            foreach (ToggleButton control in switches)
            {
                control.Checked += InputChanged;
                control.Unchecked += InputChanged;
            }
            NumberBox[] numbers = { CriticalValue, LowValue, ReminderHours, AutoUseMinutes, RunningRefresh, IdleRefresh, PreviewRemaining };
            foreach (NumberBox control in numbers)
                control.ValueChanged += delegate(object sender, NumberBoxValueChangedEventArgs e) { InputChanged(sender, null); };
            QuietHours.SelectionChanged += InputChanged;
            ThemeChoice.SelectionChanged += InputChanged;
            PreviewKeyDown += delegate(object sender, System.Windows.Input.KeyEventArgs e)
            {
                if (e.Key == System.Windows.Input.Key.Escape)
                {
                    e.Handled = true;
                    owner.Close();
                }
            };
        }

        private void InputChanged(object sender, RoutedEventArgs e)
        {
            var radio = sender as RadioButton;
            if (radio != null && radio.IsChecked != true) return;
            if (!Synchronizing && owner != null) owner.WpfInputChanged(sender);
        }

        internal void ApplyTheme(bool dark)
        {
            if (darkTheme == dark) return;
            darkTheme = dark;
            Resources.MergedDictionaries[0] = new Wpf.Ui.Markup.ThemesDictionary
            {
                Theme = dark ? Wpf.Ui.Appearance.ApplicationTheme.Dark : Wpf.Ui.Appearance.ApplicationTheme.Light
            };
            SetColor("ScreenBackground", dark ? "#14191F" : "#F4F6F8");
            SetColor("SideBackground", dark ? "#151A20" : "#EBEFF3");
            SetColor("SideBorder", dark ? "#38404A" : "#C8D0D8");
            SetColor("PrimaryText", dark ? "#F5F6F7" : "#1C242C");
            SetColor("CardBackground", dark ? "#1D2329" : "#FFFFFF");
            SetColor("CardBorder", dark ? "#47515C" : "#C3CCD5");
            SetColor("TextSecondary", dark ? "#CFD2D8" : "#4D5863");
            SetColor("IconBlue", dark ? "#68B9F8" : "#006BB5");
            SetColor("TileBackground", dark ? "#26394B" : "#E2F1FC");
            SetColor("NavSelected", dark ? "#1F4163" : "#CBE4F8");
            SetColor("NavHover", dark ? "#222A33" : "#DCE5ED");
            SetColor("ControlOutline", dark ? "#858D96" : "#7F8D9A");
            SetColor("ButtonBackground", dark ? "#232A31" : "#F4F6F8");
            SetColor("ButtonBorder", dark ? "#323B44" : "#BEC9D2");
        }

        private void SetColor(string key, string color)
        {
            Resources[key] = (System.Windows.Media.Brush)new System.Windows.Media.BrushConverter().ConvertFromString(color);
        }

        private void Navigation_SelectionChanged(object sender, SelectionChangedEventArgs e)
        {
            if (PageTitle == null || Navigation.SelectedItem == null) return;
            string page = ((ListBoxItem)Navigation.SelectedItem).Tag as string;
            UsagePanel.Visibility = page == "Usage" ? Visibility.Visible : Visibility.Collapsed;
            ForecastPanel.Visibility = page == "Forecast" ? Visibility.Visible : Visibility.Collapsed;
            GeneralPanel.Visibility = page == "General" ? Visibility.Visible : Visibility.Collapsed;
            AppearancePanel.Visibility = page == "Appearance" ? Visibility.Visible : Visibility.Collapsed;
            PopupPanel.Visibility = page == "Popup" ? Visibility.Visible : Visibility.Collapsed;
            UpdatesPanel.Visibility = page == "Updates" ? Visibility.Visible : Visibility.Collapsed;
            if (page == "General") { PageTitle.Text = "General"; PageDescription.Text = "Control refresh intervals and startup."; }
            else if (page == "Forecast") { PageTitle.Text = "Forecast & graphs"; PageDescription.Text = "Configure how usage forecasts are generated and how graphs present your usage."; }
            else if (page == "Appearance") { PageTitle.Text = "Appearance"; PageDescription.Text = "Personalize the app and tray icon."; }
            else if (page == "Popup") { PageTitle.Text = "Popup"; PageDescription.Text = "Choose which details appear in the tray popup."; }
            else if (page == "Updates") { PageTitle.Text = "Updates"; PageDescription.Text = "Check the installed version and look for updates."; }
            else { PageTitle.Text = "Usage & alerts"; PageDescription.Text = "Control how usage is shown in the tray and when you’re notified."; }
        }

        private void RestoreDefaults_Click(object sender, RoutedEventArgs e) { owner.WpfRestoreDefaults(); }
        private void Save_Click(object sender, RoutedEventArgs e) { owner.WpfSave(); }
        private void Cancel_Click(object sender, RoutedEventArgs e) { owner.Close(); }
        private void TrayBoxColor_Click(object sender, RoutedEventArgs e) { owner.WpfPickTrayBoxColor(); }
        private void TrayTextColor_Click(object sender, RoutedEventArgs e) { owner.WpfPickTrayTextColor(); }
        private void TrayBoxAuto_Click(object sender, RoutedEventArgs e) { owner.WpfUseAutomaticTrayColor(); }
        private void CheckUpdates_Click(object sender, RoutedEventArgs e) { owner.WpfCheckUpdates(); }
    }
}
