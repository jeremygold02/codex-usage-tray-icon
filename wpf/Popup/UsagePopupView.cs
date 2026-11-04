#if WPF_UI
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;
using System.Windows.Threading;
using Wpf.Ui.Controls;
using TextBlock = System.Windows.Controls.TextBlock;

namespace CodexUsageTray
{
    // The tray still owns the model, placement and lifetime. This control only
    // renders the selected provider and forwards the three header actions.
    internal sealed class UsagePopupView : UserControl
    {
        internal sealed class Row
        {
            public string Title, Reset, Forecast;
            public int Remaining;
            public Color Fill;
            public bool Weekly;
        }

        internal sealed class Reset
        {
            public string Title, Expiration;
        }

        internal sealed class State
        {
            public string Provider, Updated, Status;
            public bool CanSwitchProvider, Refreshing, Dark, Compact;
            public double?[] Daily;
            public DateTime[] DailyDates;
            public bool[] DailyPartial;
            public readonly List<Row> Rows = new List<Row>();
            public readonly List<Reset> Resets = new List<Reset>();
            public int ResetCount;
        }

        public event EventHandler RefreshRequested, SettingsRequested, HistoryRequested, ProviderRequested;
        private readonly Grid shell;
        private readonly Grid header;
        private readonly StackPanel content;
        private readonly ScrollViewer scroll;
        private TextBlock providerTitle, updatedText;
        private Wpf.Ui.Controls.Button providerAction, refreshAction;
        private StackPanel headerActions;
        private int updateVersion;
        private State state;
        private Brush text, muted, card, border, surface, track;

        public UsagePopupView()
        {
            FontFamily = new FontFamily("Segoe UI");
            TextOptions.SetTextFormattingMode(this, TextFormattingMode.Display);
            shell = new Grid();
            shell.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            shell.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
            shell.Margin = new Thickness(12, 10, 12, 12);
            header = new Grid { Margin = new Thickness(6, 0, 5, 0), VerticalAlignment = VerticalAlignment.Center };
            shell.Children.Add(header);
            content = new StackPanel();
            scroll = new ScrollViewer { Content = content, VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
                HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled, CanContentScroll = false };
            Grid.SetRow(scroll, 1);
            shell.Children.Add(scroll);
            Content = shell;
        }

        public void Update(State value)
        {
            double scrollOffset = scroll.VerticalOffset;
            int version = ++updateVersion;
            state = value;
            bool dark = value.Dark;
            surface = Brush(dark ? "#1E2125" : "#F8F9FB");
            card = Brush(dark ? "#25292E" : "#FFFFFF");
            border = Brush(dark ? "#48515B" : "#B6BEC8");
            text = Brush(dark ? "#F6F7F9" : "#171C23");
            muted = Brush(dark ? "#CDD1D7" : "#45515D");
            track = Brush(dark ? "#353A41" : "#D5DBE2");
            if (SystemParameters.HighContrast)
            {
                surface = SystemColors.WindowBrush;
                card = SystemColors.WindowBrush;
                border = SystemColors.WindowTextBrush;
                text = SystemColors.WindowTextBrush;
                muted = SystemColors.WindowTextBrush;
                track = SystemColors.ControlBrush;
            }
            Background = surface;
            Foreground = text;
            BuildHeader();
            BuildContent();
            // ScrollViewer adjusts its extent asynchronously after replacing rows.
            // Restore only the latest offset after layout, including after a resize.
            Dispatcher.BeginInvoke(DispatcherPriority.Loaded, new Action(delegate
            {
                if (version == updateVersion) scroll.ScrollToVerticalOffset(scrollOffset);
            }));
        }

        // ElementHost sizes its child to the current WinForms bounds. Measuring
        // the hosted UserControl itself would feed that old height back into the
        // next layout, so measure the unconstrained pieces instead.
        public double MeasureNaturalHeight(double width)
        {
            double innerWidth = Math.Max(1, width - shell.Margin.Left - shell.Margin.Right);
            header.Measure(new Size(innerWidth, double.PositiveInfinity));
            content.Measure(new Size(innerWidth, double.PositiveInfinity));
            return shell.Margin.Top + header.DesiredSize.Height + content.DesiredSize.Height + shell.Margin.Bottom;
        }

        private void BuildHeader()
        {
            if (header.Children.Count == 0)
            {
                header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                header.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                header.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                header.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                providerTitle = Label("", 18, text, FontWeights.Bold);
                header.Children.Add(providerTitle);
                providerAction = new Wpf.Ui.Controls.Button { FontSize = 17,
                    FontWeight = FontWeights.Bold, Padding = new Thickness(0), MinWidth = 0, MinHeight = 0,
                    BorderThickness = new Thickness(0), Background = Brushes.Transparent, Foreground = text,
                    Margin = new Thickness(0, 0, 8, 0), ToolTip = "Switch usage provider" };
                providerAction.Click += delegate { ProviderRequested?.Invoke(this, EventArgs.Empty); };
                header.Children.Add(providerAction);
                updatedText = Label("", 12, muted);
                updatedText.Margin = new Thickness(12, 0, 4, 0);
                updatedText.TextWrapping = TextWrapping.Wrap;
                Grid.SetColumn(updatedText, 1);
                header.Children.Add(updatedText);
                headerActions = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right };
                headerActions.Children.Add(Action("DataHistogram24", "Open usage history", delegate { HistoryRequested?.Invoke(this, EventArgs.Empty); }));
                refreshAction = Action("ArrowClockwise24", "Refresh usage", delegate { RefreshRequested?.Invoke(this, EventArgs.Empty); });
                headerActions.Children.Add(refreshAction);
                headerActions.Children.Add(Action("Settings24", "Open settings", delegate { SettingsRequested?.Invoke(this, EventArgs.Empty); }));
                header.Children.Add(headerActions);
            }
            providerTitle.Text = state.Provider + " Usage";
            providerTitle.Foreground = text;
            providerTitle.Visibility = state.CanSwitchProvider ? Visibility.Collapsed : Visibility.Visible;
            providerAction.Content = state.Provider + " ↔";
            providerAction.Foreground = text;
            providerAction.Visibility = state.CanSwitchProvider ? Visibility.Visible : Visibility.Collapsed;
            AutomationProperties.SetName(providerAction, "Showing " + state.Provider + " usage; switch provider");
            updatedText.Text = state.Updated ?? "";
            updatedText.Foreground = muted;
            updatedText.Visibility = string.IsNullOrEmpty(state.Updated) ? Visibility.Collapsed : Visibility.Visible;
            refreshAction.IsEnabled = !state.Refreshing;
            refreshAction.ToolTip = state.Refreshing ? "Refreshing usage" : "Refresh usage";
            AutomationProperties.SetName(refreshAction, state.Refreshing ? "Refreshing usage" : "Refresh usage");
            foreach (object item in headerActions.Children)
            {
                var button = item as Wpf.Ui.Controls.Button;
                if (button == null) continue;
                button.BorderBrush = border;
                button.Background = card;
                button.Foreground = ReferenceEquals(button, headerActions.Children[0]) && !SystemParameters.HighContrast
                    ? Brush("#69C0FF") : text;
            }
            Grid.SetRow(headerActions, state.Compact ? 1 : 0);
            Grid.SetColumn(headerActions, state.Compact ? 0 : 2);
            Grid.SetColumnSpan(headerActions, state.Compact ? 3 : 1);
        }

        private Wpf.Ui.Controls.Button Action(string icon, string name, EventHandler action)
        {
            var button = new Wpf.Ui.Controls.Button { Width = 35, Height = 28, MinWidth = 0, MinHeight = 0,
                Padding = new Thickness(0), Margin = new Thickness(8, 2, 0, 2), CornerRadius = new CornerRadius(6),
                BorderBrush = border, BorderThickness = new Thickness(1), Background = card,
                Foreground = icon == "DataHistogram24" && !SystemParameters.HighContrast ? Brush("#69C0FF") : text, ToolTip = name };
            SymbolRegular symbol;
            if (Enum.TryParse(icon, out symbol)) button.Icon = new SymbolIcon { Symbol = symbol, FontSize = 18 };
            AutomationProperties.SetName(button, name);
            button.Click += delegate { action(this, EventArgs.Empty); };
            return button;
        }

        private void BuildContent()
        {
            content.Children.Clear();
            if (state.Daily != null) content.Children.Add(HistoryCard());
            foreach (Row row in state.Rows) content.Children.Add(UsageCard(row));
            if (!string.IsNullOrEmpty(state.Status))
            {
                var status = Label(state.Status, 12, muted);
                status.TextWrapping = TextWrapping.Wrap;
                status.Margin = new Thickness(2, 10, 2, 0);
                content.Children.Add(status);
            }
            if (state.ResetCount > 0) AddResets();
        }

        private UIElement HistoryCard()
        {
            var layout = new StackPanel();
            var heading = new Grid();
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            heading.Children.Add(Label("Usage history", 12, text, FontWeights.SemiBold));
            var range = Label("Last 7 days", 11.5, muted);
            Grid.SetColumn(range, 1); heading.Children.Add(range);
            layout.Children.Add(heading);

            var bars = new UniformGrid { Columns = 7, HorizontalAlignment = HorizontalAlignment.Stretch,
                Height = 48, Margin = new Thickness(0, 6, 0, 0) };
            var details = Label("Hover or focus a day for details", 11.5, muted);
            details.Margin = new Thickness(0, 5, 0, 0);
            details.TextWrapping = TextWrapping.Wrap;
            details.Height = state.Compact ? 34 : 17;
            details.TextTrimming = TextTrimming.CharacterEllipsis;
            AutomationProperties.SetAutomationId(details, "DailyUsageDetails");
            string[] descriptions = new string[state.Daily.Length];
            int hoveredDay = -1;
            int focusedDay = -1;
            Action updateDetails = delegate
            {
                int selected = hoveredDay >= 0 ? hoveredDay : focusedDay;
                details.Text = selected >= 0 ? descriptions[selected] : "Hover or focus a day for details";
            };
            double maximum = 1;
            foreach (double? daily in state.Daily) if (daily.HasValue) maximum = Math.Max(maximum, daily.Value);
            for (int day = 0; day < state.Daily.Length; day++)
            {
                int index = day;
                double? daily = state.Daily[day];
                DateTime date = state.DailyDates[day];
                string description = UsagePopup.FormatDailyUsageDetail(date, daily, state.DailyPartial[day]);
                descriptions[day] = description;
                Brush normal = daily.HasValue ? Brush("#3E9CE6") : border;
                Brush highlighted = SystemParameters.HighContrast ? SystemColors.HighlightBrush : Brush("#83CDFF");
                var bar = new Border { Height = !daily.HasValue ? 1 : daily.Value <= 0 ? 2 : Math.Max(4, Math.Min(28, daily.Value * 28 / maximum)),
                    MaxWidth = 20, Margin = new Thickness(5, 0, 5, 0), VerticalAlignment = VerticalAlignment.Bottom,
                    CornerRadius = new CornerRadius(1), Background = normal };
                var hit = new Grid { Background = Brushes.Transparent };
                hit.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
                hit.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                hit.Children.Add(bar);
                var dayLabel = Label(date.ToString("ddd", CultureInfo.CurrentCulture), 10, muted);
                dayLabel.HorizontalAlignment = HorizontalAlignment.Center;
                dayLabel.Margin = new Thickness(0, 3, 0, 0);
                dayLabel.IsHitTestVisible = false;
                Grid.SetRow(dayLabel, 1); hit.Children.Add(dayLabel);
                var button = new System.Windows.Controls.Button { Content = hit, Padding = new Thickness(0), Margin = new Thickness(0),
                    BorderThickness = new Thickness(0), Background = Brushes.Transparent, Focusable = true,
                    MinWidth = 0, MinHeight = 0,
                    HorizontalAlignment = HorizontalAlignment.Stretch, VerticalAlignment = VerticalAlignment.Stretch,
                    Width = double.NaN, Height = double.NaN, MaxWidth = double.PositiveInfinity, MaxHeight = double.PositiveInfinity,
                    HorizontalContentAlignment = HorizontalAlignment.Stretch, VerticalContentAlignment = VerticalAlignment.Stretch };
                // A transparent template keeps the entire column as the hover
                // and keyboard target, even for a zero or unavailable day.
                var presenter = new FrameworkElementFactory(typeof(ContentPresenter));
                presenter.SetValue(ContentPresenter.HorizontalAlignmentProperty, HorizontalAlignment.Stretch);
                presenter.SetValue(ContentPresenter.VerticalAlignmentProperty, VerticalAlignment.Stretch);
                presenter.SetBinding(ContentPresenter.ContentProperty, new System.Windows.Data.Binding("Content")
                    { RelativeSource = new System.Windows.Data.RelativeSource(System.Windows.Data.RelativeSourceMode.TemplatedParent) });
                button.Template = new ControlTemplate(typeof(System.Windows.Controls.Button)) { VisualTree = presenter };
                AutomationProperties.SetName(button, description.Replace(" · ", "; "));
                button.MouseEnter += delegate { hoveredDay = index; bar.Background = highlighted; updateDetails(); };
                button.MouseLeave += delegate { if (hoveredDay == index) hoveredDay = -1; bar.Background = button.IsKeyboardFocusWithin ? highlighted : normal; updateDetails(); };
                button.GotKeyboardFocus += delegate { focusedDay = index; bar.Background = highlighted; updateDetails(); };
                button.LostKeyboardFocus += delegate { if (focusedDay == index) focusedDay = -1; bar.Background = button.IsMouseOver ? highlighted : normal; updateDetails(); };
                bars.Children.Add(button);
            }
            layout.Children.Add(bars);
            layout.Children.Add(details);
            return Card(layout, new Thickness(0, 9, 0, 0));
        }

        private UIElement UsageCard(Row row)
        {
            var layout = new StackPanel();
            var grid = new Grid { Margin = new Thickness(0, 0, 0, 8) };
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(41) });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            if (!state.Compact)
            {
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1) });
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(2, GridUnitType.Star) });
            }
            var icon = new Border { Width = 30, Height = 30, CornerRadius = new CornerRadius(6),
                Background = Brush(row.Weekly ? "#29445F" : "#574817"), HorizontalAlignment = HorizontalAlignment.Left,
                Child = new SymbolIcon { Symbol = row.Weekly ? SymbolRegular.CalendarEmpty24 : SymbolRegular.Flash24,
                    FontSize = 20, Foreground = Brush(row.Weekly ? "#6CC4FF" : "#FFDB0A"), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center } };
            grid.Children.Add(icon);
            var name = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
            name.Children.Add(Label(row.Title, 16, text, FontWeights.Bold));
            Grid.SetColumn(name, 1); grid.Children.Add(name);
            if (!state.Compact)
            {
                var separator = new Border { Background = border, Margin = new Thickness(0, 1, 0, 1) };
                Grid.SetColumn(separator, 2); grid.Children.Add(separator);
            }
            var details = new StackPanel { Margin = new Thickness(12, 0, 0, 0), VerticalAlignment = VerticalAlignment.Center };
            if (!string.IsNullOrEmpty(row.Reset)) { var l = Label(row.Reset, 11.5, muted); l.TextWrapping = TextWrapping.Wrap; details.Children.Add(l); }
            if (!string.IsNullOrEmpty(row.Forecast)) { var l = Label(row.Forecast, 11.5, muted); l.TextWrapping = TextWrapping.Wrap; details.Children.Add(l); }
            if (!state.Compact) { Grid.SetColumn(details, 3); grid.Children.Add(details); }
            layout.Children.Add(grid);
            if (state.Compact) { details.Margin = new Thickness(0, 0, 0, 8); layout.Children.Add(details); }
            int percent = Math.Max(0, Math.Min(100, row.Remaining));
            var bar = new Grid { Height = 19 };
            bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(percent, GridUnitType.Star) });
            bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(100 - percent, GridUnitType.Star) });
            var background = new Border { Background = track, CornerRadius = new CornerRadius(6) };
            Grid.SetColumnSpan(background, 2); bar.Children.Add(background);
            if (percent > 0) bar.Children.Add(new Border { Background = new SolidColorBrush(row.Fill), CornerRadius = new CornerRadius(6) });
            var number = Label(percent.ToString(CultureInfo.CurrentCulture) + "%", 12,
                percent >= 12 ? (row.Fill.R * 299 + row.Fill.G * 587 + row.Fill.B * 114 >= 150000 ? Brushes.Black : Brushes.White) : text, FontWeights.Bold);
            number.HorizontalAlignment = percent >= 12 ? HorizontalAlignment.Right : HorizontalAlignment.Left;
            number.Margin = new Thickness(5, 0, 5, 0);
            Grid.SetColumn(number, percent >= 12 ? 0 : 1); bar.Children.Add(number);
            AutomationProperties.SetName(bar, percent.ToString(CultureInfo.CurrentCulture) + " percent remaining");
            layout.Children.Add(bar);
            UIElement cardElement = Card(layout, new Thickness(0, 9, 0, 0));
            AutomationProperties.SetName(cardElement, row.Title + " limit, " +
                percent.ToString(CultureInfo.CurrentCulture) + " percent remaining");
            return cardElement;
        }

        private void AddResets()
        {
            content.Children.Add(new Border { Background = border, Height = 1, Margin = new Thickness(0, 12, 0, 10) });
            var heading = new WrapPanel { Orientation = Orientation.Horizontal };
            heading.Children.Add(new SymbolIcon { Symbol = SymbolRegular.Stack24, Foreground = Brush("#69C0FF"), FontSize = 22, Margin = new Thickness(4, 0, 10, 0) });
            heading.Children.Add(Label("Limit resets", 16, text, FontWeights.Bold));
            var count = Label("(" + state.ResetCount.ToString(CultureInfo.CurrentCulture) + " available)", 13, muted);
            count.Margin = new Thickness(8, 0, 0, 0); heading.Children.Add(count); content.Children.Add(heading);
            var list = new StackPanel();
            foreach (Reset reset in state.Resets)
            {
                if (list.Children.Count > 0) list.Children.Add(new Border { Background = border, Height = 1 });
                var row = new Grid { Margin = new Thickness(13, 4, 13, 4) };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                var title = Label(reset.Title, 12, text); title.TextWrapping = TextWrapping.Wrap; row.Children.Add(title);
                var expiration = Label(reset.Expiration, 12, muted); expiration.TextWrapping = TextWrapping.Wrap;
                expiration.TextAlignment = TextAlignment.Right;
                expiration.HorizontalAlignment = HorizontalAlignment.Right;
                expiration.Margin = new Thickness(12, 0, 0, 0); Grid.SetColumn(expiration, 1); row.Children.Add(expiration);
                list.Children.Add(row);
            }
            content.Children.Add(new Border { Child = list, Background = card, BorderBrush = border,
                BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(6), Margin = new Thickness(0, 4, 0, 0) });
        }

        private UIElement Card(UIElement child, Thickness margin)
        {
            return new Card { Content = child, Margin = margin, Padding = new Thickness(10, 8, 10, 8),
                Background = card, Foreground = text, BorderBrush = border, BorderThickness = new Thickness(1),
                HorizontalContentAlignment = HorizontalAlignment.Stretch };
        }

        private static TextBlock Label(string value, double size, Brush foreground, FontWeight? weight = null)
        {
            return new TextBlock { Text = value, FontSize = size, Foreground = foreground,
                FontWeight = weight ?? FontWeights.Normal, VerticalAlignment = VerticalAlignment.Center };
        }

        private static Brush Brush(string value) { return (Brush)new BrushConverter().ConvertFromString(value); }
    }
}
#endif
