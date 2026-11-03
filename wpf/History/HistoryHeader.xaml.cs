using System;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace CodexUsageTray
{
    public partial class HistoryHeader : UserControl
    {
        private bool updating = true;
        public event EventHandler PeriodChanged;

        public HistoryHeader()
        {
            InitializeComponent();
            updating = false;
        }

        public bool IsWeekly { get { return Weekly.IsChecked == true; } }

        public void SetValues(bool weekly, bool hasFiveHour, string remaining, string reset, string forecast)
        {
            updating = true;
            try
            {
                Weekly.IsChecked = weekly;
                FiveHour.IsChecked = !weekly;
                FiveHour.Visibility = hasFiveHour ? Visibility.Visible : Visibility.Collapsed;
                PeriodGroup.Width = hasFiveHour ? 250 : 125;
                FiveHourColumn.Width = hasFiveHour ? new GridLength(1, GridUnitType.Star) : new GridLength(0);
                Remaining.Text = remaining ?? string.Empty;
                Reset.Text = reset ?? string.Empty;
                Forecast.Text = forecast ?? string.Empty;
            }
            finally { updating = false; }
        }

        public void SetTheme(bool dark, bool highContrast)
        {
            SetBrush("HistoryBackground", highContrast ? System.Windows.SystemColors.WindowColor : dark ? Color.FromRgb(19, 24, 31) : Color.FromRgb(248, 249, 251));
            SetBrush("HistoryForeground", highContrast ? System.Windows.SystemColors.WindowTextColor : dark ? Colors.White : Colors.Black);
            SetBrush("HistorySegmentBackground", highContrast ? System.Windows.SystemColors.WindowColor : dark ? Color.FromRgb(31, 38, 48) : Colors.White);
            SetBrush("HistorySegmentBorder", highContrast ? System.Windows.SystemColors.WindowTextColor : dark ? Color.FromRgb(79, 91, 107) : Color.FromRgb(170, 178, 188));
            SetBrush("HistorySelectedBackground", highContrast ? System.Windows.SystemColors.HighlightColor : Color.FromRgb(0, 105, 190));
            SetBrush("HistorySelectedForeground", highContrast ? System.Windows.SystemColors.HighlightTextColor : Colors.White);
            SetBrush("HistoryAccent", highContrast ? System.Windows.SystemColors.WindowTextColor : dark ? Color.FromRgb(87, 185, 247) : Color.FromRgb(0, 104, 180));
            SetBrush("HistoryForecastAccent", highContrast ? System.Windows.SystemColors.WindowTextColor : dark ? Color.FromRgb(242, 178, 77) : Color.FromRgb(160, 89, 0));
        }

        private void SetBrush(string name, Color color)
        {
            Resources[name] = new SolidColorBrush(color);
        }

        private void PeriodChecked(object sender, RoutedEventArgs e)
        {
            if (!updating && PeriodChanged != null) PeriodChanged(this, EventArgs.Empty);
        }
    }
}
