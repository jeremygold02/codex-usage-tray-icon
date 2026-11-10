//! Custom-drawn controls on top of `gfx`, for windows that need more than the
//! popup's two click targets: check boxes, switches, radio buttons, segmented
//! choices, dropdowns, numeric fields, color swatches, push buttons, setting
//! cards and a scroll bar.
//!
//! Nothing here owns a window. A window describes its content as a tree of
//! [`Node`]s each time it lays out; the same pass measures, paints and records
//! where every control ended up, so hit testing and the keyboard focus order
//! always match what is drawn. [`Interaction`] keeps the pointer and focus
//! state between passes. Sizes given to this module are in pixels at 100%
//! scale unless a name says otherwise; rectangles are in device pixels.

use crate::gfx::{Align, Font, Graphics, Rect};
use crate::render::Theme;

const UI_FONTS: [&str; 3] = ["Segoe UI", "Tahoma", "Microsoft Sans Serif"];
const SEMIBOLD_FONTS: [&str; 1] = ["Segoe UI Semibold"];
const ICON_FONTS: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];

/// How strongly a disabled control keeps its own colors over the backdrop.
const DISABLED_STRENGTH: f32 = 0.55;
const CHECK_SIDE: f32 = 18.0;
const CHECK_LABEL_OFFSET: f32 = 28.0;
const TOGGLE_WIDTH: f32 = 40.0;
const TOGGLE_HEIGHT: f32 = 20.0;
const TOGGLE_LABEL_GAP: f32 = 10.0;
const NUMBER_WIDTH: f32 = 96.0;
const NUMBER_HEIGHT: f32 = 35.0;
const NUMBER_TEXT_INSET: f32 = 11.0;
/// Width of the strip at the right of a numeric field that steps the value.
const NUMBER_SPIN_WIDTH: f32 = 28.0;
const DROPDOWN_HEIGHT: f32 = 39.0;
const DROPDOWN_ITEM_HEIGHT: f32 = 37.0;
const DROPDOWN_TEXT_INSET: f32 = 12.0;
const SWATCH_SIZE: (f32, f32) = (38.0, 32.0);
const SEGMENT_PADDING: (f32, f32) = (12.0, 8.0);
const SEGMENT_INSET: f32 = 3.0;
const CARD_PADDING: (f32, f32) = (24.0, 12.0);
const CARD_ICON_COLUMN: f32 = 68.0;
const CARD_TILE: f32 = 48.0;
const CARD_RADIUS: f32 = 10.0;
/// Below this text width a card puts its options under the text instead.
const CARD_MIN_TEXT_WIDTH: f32 = 190.0;
const CARD_COLUMN_GAP: f32 = 12.0;
const SPREAD_GAP: f32 = 16.0;
/// The longest number a numeric field accepts, which always fits an `i64`.
const MAX_NUMBER_LENGTH: usize = 9;

/// Blends two ARGB colors; `amount` is how much of `to` the result takes.
pub fn mix(from: u32, to: u32, amount: f32) -> u32 {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |shift: u32| {
        let start = ((from >> shift) & 0xFF) as f32;
        let end = ((to >> shift) & 0xFF) as f32;
        ((start + (end - start) * amount).round() as u32) << shift
    };
    channel(24) | channel(16) | channel(8) | channel(0)
}

/// The popup's palette plus the shades controls need, derived from it.
pub struct Palette {
    pub dark: bool,
    pub surface: u32,
    pub card: u32,
    pub hover: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    /// Outline of unchecked boxes, fields and dropdowns.
    pub outline: u32,
    /// Fill of fields and ordinary buttons.
    pub field: u32,
    pub field_hover: u32,
    pub field_pressed: u32,
    /// Fill of the selected item in a list.
    pub selected: u32,
    /// Background of the tile behind a card's icon.
    pub tile: u32,
    /// Fill of checked boxes, switches that are on and the default button.
    pub primary: u32,
    pub primary_hover: u32,
    pub primary_pressed: u32,
    pub primary_edge: u32,
    pub on_primary: u32,
    /// Text that warns, such as a low-contrast notice.
    pub warning: u32,
}

impl Palette {
    /// The palette of the dark or the light theme.
    pub fn new(dark: bool) -> Self {
        let theme = Theme::new(dark);
        let primary = 0xFF00_78D7;
        Self {
            dark,
            surface: theme.surface,
            card: theme.card,
            hover: theme.hover,
            border: theme.border,
            text: theme.text,
            muted: theme.muted,
            accent: theme.accent,
            outline: mix(theme.border, theme.muted, 0.5),
            field: mix(theme.card, theme.text, 0.05),
            field_hover: mix(theme.card, theme.text, 0.11),
            field_pressed: mix(theme.card, theme.text, 0.02),
            selected: mix(theme.surface, theme.accent, if dark { 0.24 } else { 0.2 }),
            tile: mix(theme.card, theme.accent, if dark { 0.18 } else { 0.13 }),
            primary,
            primary_hover: mix(primary, 0xFFFF_FFFF, 0.12),
            primary_pressed: mix(primary, 0xFF00_0000, 0.14),
            primary_edge: 0xFF16_8EFA,
            on_primary: 0xFFFF_FFFF,
            warning: if dark { 0xFFFF_CD56 } else { 0xFF8B_0000 },
        }
    }
}

/// The fonts controls and their surroundings are drawn with.
pub struct Fonts {
    pub body: Font,
    /// Body size, semibold.
    pub strong: Font,
    /// Card titles.
    pub title: Font,
    /// The page heading.
    pub heading: Font,
    /// The line under the page heading.
    pub lead: Font,
    pub navigation: Font,
    icon_small: Option<Font>,
    icon_navigation: Option<Font>,
    icon_tile: Option<Font>,
}

impl Fonts {
    fn new(scale: f32) -> Option<Self> {
        let regular = |size: f32| Font::new(&UI_FONTS, size * scale, false);
        let semibold = |size: f32| {
            Font::new(&SEMIBOLD_FONTS, size * scale, false)
                .or_else(|| Font::new(&UI_FONTS, size * scale, true))
        };
        let icons = |size: f32| Font::new(&ICON_FONTS, size * scale, false);
        Some(Self {
            body: regular(16.0)?,
            strong: semibold(16.0)?,
            title: semibold(19.0)?,
            heading: Font::new(&UI_FONTS, 33.0 * scale, true)?,
            lead: regular(18.0)?,
            navigation: regular(17.0)?,
            icon_small: icons(IconSize::Small.points()),
            icon_navigation: icons(IconSize::Navigation.points()),
            icon_tile: icons(IconSize::Tile.points()),
        })
    }
}

/// Colors, fonts and scale for one window at one DPI.
pub struct Style {
    pub palette: Palette,
    pub fonts: Fonts,
    pub scale: f32,
}

impl Style {
    /// Returns `None` when no usable text font is installed.
    pub fn new(scale: f32, dark: bool) -> Option<Self> {
        Some(Self {
            palette: Palette::new(dark),
            fonts: Fonts::new(scale)?,
            scale,
        })
    }

    /// Converts a length at 100% scale to whole device pixels.
    pub fn px(&self, value: f32) -> f32 {
        (value * self.scale).round()
    }

    /// The width of a one-pixel line at this scale, never thinner than a pixel.
    pub fn hairline(&self) -> f32 {
        self.px(1.0).max(1.0)
    }

    /// The color to draw with, faded toward `backdrop` when disabled.
    fn tone(&self, color: u32, backdrop: u32, enabled: bool) -> u32 {
        if enabled {
            color
        } else {
            mix(backdrop, color, DISABLED_STRENGTH)
        }
    }

    fn line_height(&self, g: &Graphics, font: &Font) -> f32 {
        g.measure("Ag", font).1
    }
}

/// A symbol shown beside a navigation entry or on a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// A character of the Windows icon font.
    Glyph(&'static str),
    /// Three bars of a chart, which the icon font has no outline glyph for.
    Bars,
}

/// The sizes icons are drawn at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconSize {
    Small,
    Navigation,
    Tile,
}

impl IconSize {
    const fn points(self) -> f32 {
        match self {
            Self::Small => 21.0,
            Self::Navigation => 23.0,
            Self::Tile => 27.0,
        }
    }
}

/// Draws `icon` centered in `rect`.
pub fn icon(g: &Graphics, style: &Style, icon: Icon, size: IconSize, color: u32, rect: Rect) {
    match icon {
        Icon::Glyph(glyph) => {
            let font = match size {
                IconSize::Small => &style.fonts.icon_small,
                IconSize::Navigation => &style.fonts.icon_navigation,
                IconSize::Tile => &style.fonts.icon_tile,
            };
            if let Some(font) = font {
                g.text(glyph, font, color, rect, Align::Center, Align::Center);
            }
        }
        Icon::Bars => {
            let side = (size.points() * style.scale * 0.8).round();
            let left = (rect.x + (rect.w - side) / 2.0).round();
            let bottom = (rect.y + (rect.h + side) / 2.0).round();
            let stroke = (side / 13.0).max(1.0);
            let bar = (side * 0.24).round();
            let step = (side - bar) / 2.0;
            for (index, height) in [0.5, 1.0, 0.72].into_iter().enumerate() {
                let height = (side * height).round();
                let bar_rect = Rect::new(
                    (left + step * index as f32).round(),
                    bottom - height,
                    bar,
                    height,
                );
                g.stroke_round_rect_wide(color, bar_rect, stroke * 1.5, stroke);
            }
        }
    }
}

/// How a control should look right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Visual {
    pub hovered: bool,
    pub pressed: bool,
    /// Has the keyboard focus, and the keyboard is being used to move it.
    pub focused: bool,
    pub enabled: bool,
}

/// The mouse pointer a control asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Hand,
    Text,
}

/// Where a control ended up in the last layout pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target<Id> {
    pub id: Id,
    /// The control's whole rectangle, even where it is scrolled out of view.
    pub bounds: Rect,
    /// The part that can be seen and clicked.
    pub visible: Option<Rect>,
    pub enabled: bool,
    /// Whether Tab stops here.
    pub tab_stop: bool,
    pub cursor: Cursor,
}

/// Pointer and keyboard-focus state for the controls of one window.
pub struct Interaction<Id> {
    targets: Vec<Target<Id>>,
    hovered: Option<Id>,
    pressed: Option<Id>,
    focused: Option<Id>,
    /// Focus is only outlined once the keyboard has been used to move it.
    focus_visible: bool,
}

impl<Id: Copy + PartialEq> Default for Interaction<Id> {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            hovered: None,
            pressed: None,
            focused: None,
            focus_visible: false,
        }
    }
}

impl<Id: Copy + PartialEq> Interaction<Id> {
    /// Forgets the previous layout's targets; call before laying out.
    pub fn begin_layout(&mut self) {
        self.targets.clear();
    }

    /// Records a control. Later controls are on top of earlier ones, and the
    /// order of tab stops is the order they are added in.
    pub fn add(&mut self, target: Target<Id>) {
        self.targets.push(target);
    }

    /// Drops state that refers to controls the new layout no longer offers.
    pub fn end_layout(&mut self) {
        let usable = |targets: &[Target<Id>], id: Option<Id>| {
            id.filter(|id| {
                targets
                    .iter()
                    .any(|target| target.id == *id && target.enabled)
            })
        };
        self.hovered = usable(&self.targets, self.hovered);
        self.pressed = usable(&self.targets, self.pressed);
        self.focused = usable(&self.targets, self.focused);
    }

    /// Every control of the last layout, in the order they were added.
    #[cfg(test)]
    pub fn targets(&self) -> &[Target<Id>] {
        &self.targets
    }

    /// Where a control was in the last layout, if it was part of it.
    pub fn target(&self, id: Id) -> Option<&Target<Id>> {
        self.targets.iter().rev().find(|target| target.id == id)
    }

    /// The topmost enabled control under a point.
    pub fn target_at(&self, x: f32, y: f32) -> Option<&Target<Id>> {
        self.targets.iter().rev().find(|target| {
            target.enabled && target.visible.is_some_and(|visible| visible.contains(x, y))
        })
    }

    /// The mouse pointer to show at a point.
    pub fn cursor_at(&self, x: f32, y: f32) -> Cursor {
        self.target_at(x, y)
            .map_or(Cursor::Arrow, |target| target.cursor)
    }

    /// The control under the pointer.
    pub fn hovered(&self) -> Option<Id> {
        self.hovered
    }

    /// The control with the keyboard focus.
    pub fn focused(&self) -> Option<Id> {
        self.focused
    }

    /// The pointer and focus part of a control's look.
    pub fn visual(&self, id: Id, enabled: bool) -> Visual {
        let over = self.hovered == Some(id);
        Visual {
            // While a button is held on one control the others do not react.
            hovered: enabled && over && self.pressed.is_none_or(|pressed| pressed == id),
            pressed: enabled && over && self.pressed == Some(id),
            focused: enabled && self.focus_visible && self.focused == Some(id),
            enabled,
        }
    }

    /// Updates the hovered control; `None` means the pointer left the window.
    /// Returns true when the hovered control changed.
    pub fn pointer_moved(&mut self, point: Option<(f32, f32)>) -> bool {
        let hovered = point.and_then(|(x, y)| self.target_at(x, y).map(|target| target.id));
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    /// Starts a click and gives the control under the pointer the focus.
    pub fn pointer_pressed(&mut self, x: f32, y: f32) -> Option<Id> {
        self.pointer_moved(Some((x, y)));
        self.pressed = self.hovered;
        if let Some(id) = self.pressed {
            self.focused = Some(id);
            self.focus_visible = false;
        }
        self.pressed
    }

    /// Ends a click. Returns the control when the button was released over
    /// the control it was pressed on.
    pub fn pointer_released(&mut self, x: f32, y: f32) -> Option<Id> {
        self.pointer_moved(Some((x, y)));
        let pressed = self.pressed.take();
        pressed.filter(|id| self.hovered == Some(*id))
    }

    /// Abandons a click, for when the window loses the mouse.
    pub fn cancel_press(&mut self) {
        self.pressed = None;
    }

    /// Moves the focus; `visible` shows the focus outline.
    pub fn set_focus(&mut self, id: Option<Id>, visible: bool) {
        self.focused = id;
        self.focus_visible = visible;
    }

    /// Shows the focus outline, for when the keyboard starts being used.
    pub fn show_focus(&mut self) {
        self.focus_visible = true;
    }

    /// Moves the focus to the next or previous tab stop, wrapping around.
    pub fn focus_next(&mut self, backward: bool) -> Option<Id> {
        let stops: Vec<Id> = self
            .targets
            .iter()
            .filter(|target| target.enabled && target.tab_stop)
            .map(|target| target.id)
            .collect();
        let current = self
            .focused
            .and_then(|focused| stops.iter().position(|id| *id == focused));
        let next = match (current, backward) {
            (_, _) if stops.is_empty() => return None,
            (Some(index), false) => (index + 1) % stops.len(),
            (Some(index), true) => (index + stops.len() - 1) % stops.len(),
            (None, false) => 0,
            (None, true) => stops.len() - 1,
        };
        self.focused = Some(stops[next]);
        self.focus_visible = true;
        self.focused
    }
}

/// Where the caret goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
    Home,
    End,
}

/// The text being typed into a numeric field before it becomes the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberEdit {
    text: String,
    caret: usize,
    all_selected: bool,
}

impl NumberEdit {
    /// Starts editing `value`, with the caret at the end.
    pub fn new(value: i32, select_all: bool) -> Self {
        let text = value.to_string();
        Self {
            caret: text.len(),
            text,
            all_selected: select_all,
        }
    }

    /// What has been typed so far.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The caret's position, as the number of characters before it.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// True while the whole text is selected, so typing replaces it.
    pub fn all_selected(&self) -> bool {
        self.all_selected
    }

    /// Types one character. Only digits are accepted, and a leading minus
    /// when `allow_negative`. Typing replaces a selection.
    pub fn insert(&mut self, character: char, allow_negative: bool) -> bool {
        let minus = character == '-' && allow_negative;
        if !(character.is_ascii_digit() || minus) {
            return false;
        }
        let (text, caret) = if self.all_selected {
            ("", 0)
        } else {
            (self.text.as_str(), self.caret)
        };
        let before_sign = caret == 0 && text.starts_with('-');
        if text.len() >= MAX_NUMBER_LENGTH || before_sign || (minus && caret != 0) {
            return false;
        }
        let mut text = text.to_string();
        text.insert(caret, character);
        self.text = text;
        self.caret = caret + 1;
        self.all_selected = false;
        true
    }

    /// Deletes the selection, or the character before the caret.
    pub fn backspace(&mut self) {
        if self.all_selected {
            self.clear();
        } else if self.caret > 0 {
            self.caret -= 1;
            self.text.remove(self.caret);
        }
    }

    /// Deletes the selection, or the character after the caret.
    pub fn delete(&mut self) {
        if self.all_selected {
            self.clear();
        } else if self.caret < self.text.len() {
            self.text.remove(self.caret);
        }
    }

    /// Moves the caret and drops the selection.
    pub fn move_caret(&mut self, to: CaretMove) {
        self.caret = match to {
            CaretMove::Home => 0,
            CaretMove::End => self.text.len(),
            // Leaving a selection puts the caret at the end moved toward.
            CaretMove::Left if self.all_selected => 0,
            CaretMove::Right if self.all_selected => self.text.len(),
            CaretMove::Left => self.caret.saturating_sub(1),
            CaretMove::Right => (self.caret + 1).min(self.text.len()),
        };
        self.all_selected = false;
    }

    /// Puts the caret before the character at `index`.
    pub fn set_caret(&mut self, index: usize) {
        self.caret = index.min(self.text.len());
        self.all_selected = false;
    }

    /// Selects the whole text, if there is any.
    pub fn select_all(&mut self) {
        self.caret = self.text.len();
        self.all_selected = !self.text.is_empty();
    }

    /// The number typed, brought into `min..=max`, or `None` when the text
    /// is not a number.
    pub fn value(&self, min: i32, max: i32) -> Option<i32> {
        let typed: i64 = self.text.parse().ok()?;
        i32::try_from(typed.clamp(i64::from(min), i64::from(max))).ok()
    }

    /// True when the text does not say `value`, so committing or abandoning
    /// the edit would change what is shown.
    pub fn differs_from(&self, value: i32) -> bool {
        self.text.parse::<i64>().ok() != Some(i64::from(value))
    }

    fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
        self.all_selected = false;
    }
}

/// The position of content taller than the area it is shown in. Lengths are
/// in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    offset: f32,
    content: f32,
    viewport: f32,
}

impl Scroll {
    /// How far the content is scrolled up.
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// Sets the content and viewport heights, keeping the offset in range.
    pub fn set_extent(&mut self, content: f32, viewport: f32) {
        self.content = content.max(0.0);
        self.viewport = viewport.max(0.0);
        self.offset = self.offset.clamp(0.0, self.max_offset());
    }

    /// The offset that shows the end of the content.
    pub fn max_offset(&self) -> f32 {
        (self.content - self.viewport).max(0.0)
    }

    /// True when the content is taller than the area it is shown in.
    pub fn is_scrollable(&self) -> bool {
        self.max_offset() > 0.0
    }

    /// Scrolls to `offset` as far as the content allows. Returns true when
    /// the position changed.
    pub fn scroll_to(&mut self, offset: f32) -> bool {
        let offset = offset.clamp(0.0, self.max_offset());
        let changed = offset != self.offset;
        self.offset = offset;
        changed
    }

    /// Scrolls down by `delta`, or up when it is negative. Returns true when
    /// the position changed.
    pub fn scroll_by(&mut self, delta: f32) -> bool {
        self.scroll_to(self.offset + delta)
    }

    /// Scrolls just far enough to show `top..bottom`, given in content
    /// coordinates. Returns true when the position changed.
    pub fn reveal(&mut self, top: f32, bottom: f32) -> bool {
        if top < self.offset {
            self.scroll_to(top)
        } else if bottom > self.offset + self.viewport {
            // Something taller than the viewport is shown from its top.
            self.scroll_to((bottom - self.viewport).min(top))
        } else {
            false
        }
    }

    /// The scroll bar's thumb inside `track`, when there is anything to scroll.
    pub fn thumb(&self, track: Rect, min_length: f32) -> Option<Rect> {
        if !self.is_scrollable() || track.h <= 0.0 {
            return None;
        }
        let length = (track.h * self.viewport / self.content)
            .max(min_length)
            .min(track.h);
        let travel = track.h - length;
        let top = track.y + travel * self.offset / self.max_offset();
        Some(Rect::new(track.x, top.round(), track.w, length.round()))
    }

    /// The offset that puts the thumb's top edge at `thumb_top`.
    pub fn offset_for_thumb(&self, track: Rect, min_length: f32, thumb_top: f32) -> f32 {
        let Some(thumb) = self.thumb(track, min_length) else {
            return 0.0;
        };
        let travel = track.h - thumb.h;
        if travel <= 0.0 {
            return 0.0;
        }
        ((thumb_top - track.y) / travel).clamp(0.0, 1.0) * self.max_offset()
    }
}

/// The state shared by one layout-and-paint pass over a group of controls.
pub struct Painter<'a, Id> {
    pub g: &'a Graphics,
    pub style: &'a Style,
    pub interaction: &'a mut Interaction<Id>,
    /// False to lay out and record targets without drawing.
    pub paint: bool,
    /// The visible part of a scrolled area; controls cannot be clicked
    /// outside it.
    pub clip: Option<Rect>,
    /// The color behind the controls, which disabled ones fade toward.
    pub backdrop: u32,
}

impl<Id: Copy + PartialEq> Painter<'_, Id> {
    /// Records a control and returns how it should look.
    pub fn target(
        &mut self,
        id: Id,
        bounds: Rect,
        enabled: bool,
        tab_stop: bool,
        cursor: Cursor,
    ) -> Visual {
        let visible = match &self.clip {
            Some(clip) => bounds.intersection(clip),
            None => Some(bounds),
        };
        self.interaction.add(Target {
            id,
            bounds,
            visible,
            enabled,
            tab_stop,
            cursor,
        });
        self.interaction.visual(id, enabled)
    }
}

/// The color role of a piece of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Text,
    Warning,
}

/// How a column places children narrower than itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Start,
    Stretch,
}

/// The caret and selection shown in a numeric field that is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub index: usize,
    pub all_selected: bool,
    /// False during the dark half of the caret's blink.
    pub visible: bool,
}

/// Something that paints itself into the rectangle it was measured for.
pub type CustomPaint<'a> = Box<dyn Fn(&Graphics, &Style, Rect) + 'a>;

/// One piece of a control layout.
pub enum Node<'a, Id> {
    /// Text; `wrap` is the width at which it wraps, if it may.
    Text {
        text: String,
        tone: Tone,
        wrap: Option<f32>,
    },
    Checkbox {
        id: Id,
        label: &'a str,
        checked: bool,
        enabled: bool,
    },
    /// One choice of a group; the selected one is the group's tab stop.
    Radio {
        id: Id,
        label: &'a str,
        selected: bool,
        enabled: bool,
    },
    /// A switch with an optional label after it.
    Toggle {
        id: Id,
        label: Option<&'a str>,
        on: bool,
        enabled: bool,
    },
    Number {
        id: Id,
        text: String,
        caret: Option<Caret>,
        enabled: bool,
    },
    Dropdown {
        id: Id,
        text: &'a str,
        width: f32,
        open: bool,
        enabled: bool,
    },
    /// Choices side by side in one frame, of equal width.
    Segments {
        options: Vec<(Id, &'a str)>,
        selected: usize,
    },
    Swatch {
        id: Id,
        color: u32,
        enabled: bool,
    },
    Button {
        id: Id,
        label: String,
        /// Space around the label, horizontally and vertically.
        padding: (f32, f32),
        primary: bool,
        enabled: bool,
    },
    /// Anything else, of a fixed size at 100% scale.
    Custom {
        size: (f32, f32),
        paint: CustomPaint<'a>,
    },
    /// Children left to right, centered vertically.
    Row {
        gap: f32,
        children: Vec<Node<'a, Id>>,
    },
    /// Children top to bottom.
    Column {
        gap: f32,
        align: HAlign,
        children: Vec<Node<'a, Id>>,
    },
    /// Two children pushed to the left and right edges of the space given.
    Spread(Box<Node<'a, Id>>, Box<Node<'a, Id>>),
    /// A child shifted right.
    Indent(f32, Box<Node<'a, Id>>),
}

impl<Id: Copy + PartialEq> Node<'_, Id> {
    /// A line of ordinary text.
    pub fn label(text: &str) -> Self {
        Self::Text {
            text: text.to_string(),
            tone: Tone::Text,
            wrap: None,
        }
    }

    /// The node's natural size in device pixels.
    pub fn measure(&self, g: &Graphics, style: &Style) -> (f32, f32) {
        match self {
            Self::Text { text, wrap, .. } => {
                let (width, height) = g.measure(text, &style.fonts.body);
                match wrap.map(|wrap| style.px(wrap)) {
                    Some(limit) if width > limit => {
                        (limit, g.measure_wrapped(text, &style.fonts.body, limit))
                    }
                    _ => (width, height),
                }
            }
            Self::Checkbox { label, .. } | Self::Radio { label, .. } => {
                let (width, height) = g.measure(label, &style.fonts.body);
                (
                    style.px(CHECK_LABEL_OFFSET) + width,
                    height.max(style.px(CHECK_SIDE + 2.0)),
                )
            }
            Self::Toggle { label, .. } => {
                let (width, height) = label.map_or((0.0, 0.0), |label| {
                    let (width, height) = g.measure(label, &style.fonts.body);
                    (style.px(TOGGLE_LABEL_GAP) + width, height)
                });
                (
                    style.px(TOGGLE_WIDTH) + width,
                    height.max(style.px(TOGGLE_HEIGHT)),
                )
            }
            Self::Number { .. } => (style.px(NUMBER_WIDTH), style.px(NUMBER_HEIGHT)),
            Self::Dropdown { width, .. } => (style.px(*width), style.px(DROPDOWN_HEIGHT)),
            Self::Segments { options, .. } => {
                let (mut widest, mut tallest) = (0.0f32, 0.0f32);
                for (_, label) in options {
                    let (width, height) = g.measure(label, &style.fonts.strong);
                    widest = widest.max(width);
                    tallest = tallest.max(height);
                }
                let frame = 2.0 * (style.px(SEGMENT_INSET) + style.hairline());
                (
                    (widest + 2.0 * style.px(SEGMENT_PADDING.0)) * options.len() as f32 + frame,
                    tallest + 2.0 * style.px(SEGMENT_PADDING.1) + frame,
                )
            }
            Self::Swatch { .. } => (style.px(SWATCH_SIZE.0), style.px(SWATCH_SIZE.1)),
            Self::Button { label, padding, .. } => {
                let (width, height) = g.measure(label, &style.fonts.body);
                let border = 2.0 * style.hairline();
                (
                    width + 2.0 * style.px(padding.0) + border,
                    height + 2.0 * style.px(padding.1) + border,
                )
            }
            Self::Custom { size, .. } => (style.px(size.0), style.px(size.1)),
            Self::Row { gap, children } => {
                let gaps = style.px(*gap) * children.len().saturating_sub(1) as f32;
                children
                    .iter()
                    .map(|child| child.measure(g, style))
                    .fold((gaps, 0.0f32), |(width, height), (w, h)| {
                        (width + w, height.max(h))
                    })
            }
            Self::Column { gap, children, .. } => {
                let gaps = style.px(*gap) * children.len().saturating_sub(1) as f32;
                children
                    .iter()
                    .map(|child| child.measure(g, style))
                    .fold((0.0f32, gaps), |(width, height), (w, h)| {
                        (width.max(w), height + h)
                    })
            }
            Self::Spread(left, right) => {
                let (left, right) = (left.measure(g, style), right.measure(g, style));
                (left.0 + style.px(SPREAD_GAP) + right.0, left.1.max(right.1))
            }
            Self::Indent(indent, child) => {
                let (width, height) = child.measure(g, style);
                (style.px(*indent) + width, height)
            }
        }
    }

    /// Lays the node out from the top-left of `area`, records its controls
    /// and, when the painter paints, draws it. Nodes that can stretch use the
    /// width of `area`; the rest keep their natural size.
    pub fn place(&self, painter: &mut Painter<'_, Id>, area: Rect) {
        let (g, style) = (painter.g, painter.style);
        let (width, height) = self.measure(g, style);
        let bounds = Rect::new(area.x, area.y, width, height);
        let palette = &style.palette;
        match self {
            Self::Text { text, tone, wrap } => {
                if !painter.paint {
                    return;
                }
                let color = match tone {
                    Tone::Text => palette.text,
                    Tone::Warning => palette.warning,
                };
                if wrap.is_some() {
                    let rect = Rect::new(bounds.x, bounds.y, bounds.w, 0.0);
                    g.text_wrapped(text, &style.fonts.body, color, rect);
                } else {
                    let rect = Rect::new(bounds.x, bounds.y, 0.0, bounds.h);
                    g.text(
                        text,
                        &style.fonts.body,
                        color,
                        rect,
                        Align::Near,
                        Align::Center,
                    );
                }
            }
            Self::Checkbox {
                id,
                label,
                checked,
                enabled,
            } => {
                let visual = painter.target(*id, bounds, *enabled, true, Cursor::Hand);
                if painter.paint {
                    checkbox(g, style, painter.backdrop, bounds, label, *checked, visual);
                }
            }
            Self::Radio {
                id,
                label,
                selected,
                enabled,
            } => {
                let visual = painter.target(*id, bounds, *enabled, *selected, Cursor::Hand);
                if painter.paint {
                    radio(g, style, painter.backdrop, bounds, label, *selected, visual);
                }
            }
            Self::Toggle {
                id,
                label,
                on,
                enabled,
            } => {
                let visual = painter.target(*id, bounds, *enabled, true, Cursor::Hand);
                if painter.paint {
                    toggle(g, style, painter.backdrop, bounds, *label, *on, visual);
                }
            }
            Self::Number {
                id,
                text,
                caret,
                enabled,
            } => {
                let visual = painter.target(*id, bounds, *enabled, true, Cursor::Text);
                if painter.paint {
                    number_field(g, style, painter.backdrop, bounds, text, *caret, visual);
                }
            }
            Self::Dropdown {
                id,
                text,
                open,
                enabled,
                ..
            } => {
                let visual = painter.target(*id, bounds, *enabled, true, Cursor::Hand);
                if painter.paint {
                    dropdown_field(g, style, painter.backdrop, bounds, text, *open, visual);
                }
            }
            Self::Segments { options, selected } => {
                let frame = Rect::new(area.x, area.y, width.max(area.w), height);
                let rects = segment_rects(style, frame, options.len());
                let visuals: Vec<Visual> = options
                    .iter()
                    .zip(&rects)
                    .enumerate()
                    .map(|(index, ((id, _), rect))| {
                        painter.target(*id, *rect, true, index == *selected, Cursor::Hand)
                    })
                    .collect();
                if painter.paint {
                    let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
                    segments(g, style, frame, &labels, *selected, &visuals);
                }
            }
            Self::Swatch { id, color, enabled } => {
                let visual = painter.target(*id, bounds, *enabled, true, Cursor::Hand);
                if painter.paint {
                    swatch(g, style, painter.backdrop, bounds, *color, visual);
                }
            }
            Self::Button {
                id,
                label,
                primary,
                enabled,
                ..
            } => {
                let visual = painter.target(*id, bounds, *enabled, true, Cursor::Hand);
                if painter.paint {
                    button(g, style, painter.backdrop, bounds, label, *primary, visual);
                }
            }
            Self::Custom { paint, .. } => {
                if painter.paint {
                    paint(g, style, bounds);
                }
            }
            Self::Row { gap, children } => {
                let mut x = area.x;
                for child in children {
                    let (child_width, child_height) = child.measure(g, style);
                    let y = area.y + ((height - child_height) / 2.0).round();
                    child.place(painter, Rect::new(x, y, child_width, child_height));
                    x += child_width + style.px(*gap);
                }
            }
            Self::Column {
                gap,
                align,
                children,
            } => {
                let column_width = width.max(area.w);
                let mut y = area.y;
                for child in children {
                    let (child_width, child_height) = child.measure(g, style);
                    let rect = match align {
                        HAlign::Start => Rect::new(area.x, y, child_width, child_height),
                        HAlign::Stretch => Rect::new(area.x, y, column_width, child_height),
                    };
                    child.place(painter, rect);
                    y += child_height + style.px(*gap);
                }
            }
            Self::Spread(left, right) => {
                let spread_width = width.max(area.w);
                let (left_size, right_size) = (left.measure(g, style), right.measure(g, style));
                let center = |size: (f32, f32)| area.y + ((height - size.1) / 2.0).round();
                left.place(
                    painter,
                    Rect::new(area.x, center(left_size), left_size.0, left_size.1),
                );
                right.place(
                    painter,
                    Rect::new(
                        area.x + spread_width - right_size.0,
                        center(right_size),
                        right_size.0,
                        right_size.1,
                    ),
                );
            }
            Self::Indent(indent, child) => {
                let indent = style.px(*indent);
                child.place(
                    painter,
                    Rect::new(area.x + indent, area.y, width - indent, height),
                );
            }
        }
    }
}

/// How a card divides its width between its text and its options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Split {
    /// Proportional shares of the space beside the icon.
    Share { text: f32, options: f32 },
    /// A fixed options column; the text takes the rest.
    Options(f32),
}

/// A bordered section with an icon, a heading, a description and controls.
pub struct Card<'a, Id> {
    pub icon: Icon,
    pub title: &'a str,
    pub description: &'a str,
    pub min_height: f32,
    pub split: Split,
    /// `None` puts the options at the right edge at their natural width;
    /// `Some(margin)` gives them their whole column after a left margin.
    pub fill: Option<f32>,
    pub options: Node<'a, Id>,
}

/// Where the parts of a card go, relative to the card's top-left corner.
struct Arrangement {
    height: f32,
    tile: Rect,
    text: Rect,
    title_height: f32,
    options: Rect,
}

impl<Id: Copy + PartialEq> Card<'_, Id> {
    fn arrange(&self, g: &Graphics, style: &Style, width: f32) -> Arrangement {
        let pad_x = style.px(CARD_PADDING.0) + style.hairline();
        let pad_y = style.px(CARD_PADDING.1) + style.hairline();
        let icon_column = style.px(CARD_ICON_COLUMN);
        let beside_icon = (width - 2.0 * pad_x - icon_column).max(0.0);
        let gap = style.px(CARD_COLUMN_GAP);
        let (natural_width, options_height) = self.options.measure(g, style);
        let column = match self.split {
            Split::Share { text, options } => (beside_icon * options / (text + options)).round(),
            Split::Options(width) => style.px(width),
        };
        let (wanted, margin) = match self.fill {
            Some(margin) => {
                let margin = style.px(margin);
                (natural_width.max(column - margin), margin)
            }
            None => (natural_width, gap),
        };
        let side_by_side = beside_icon - column.max(wanted + margin);
        let stacked = side_by_side < style.px(CARD_MIN_TEXT_WIDTH);
        let text_width = if stacked { beside_icon } else { side_by_side };
        let options_width = if stacked && self.fill.is_some() {
            beside_icon.max(natural_width)
        } else {
            wanted
        };

        let title_height = g.measure_wrapped(self.title, &style.fonts.title, text_width);
        let description_height =
            g.measure_wrapped(self.description, &style.fonts.body, text_width - gap);
        let text_height = title_height + style.px(3.0) + description_height;
        let tile = style.px(CARD_TILE);
        let content_height = if stacked {
            text_height.max(tile) + gap + options_height
        } else {
            text_height.max(options_height).max(tile)
        };
        let height = style.px(self.min_height).max(content_height + 2.0 * pad_y);
        let text_x = pad_x + icon_column;
        if stacked {
            let text_block = text_height.max(tile);
            let top = ((height - content_height) / 2.0).round();
            Arrangement {
                height,
                tile: Rect::new(pad_x, top + ((text_block - tile) / 2.0).round(), tile, tile),
                text: Rect::new(
                    text_x,
                    top + ((text_block - text_height) / 2.0).round(),
                    text_width,
                    text_height,
                ),
                title_height,
                options: Rect::new(
                    text_x,
                    top + text_block + gap,
                    options_width,
                    options_height,
                ),
            }
        } else {
            let center = |part: f32| ((height - part) / 2.0).round();
            Arrangement {
                height,
                tile: Rect::new(pad_x, center(tile), tile, tile),
                text: Rect::new(text_x, center(text_height), text_width, text_height),
                title_height,
                options: Rect::new(
                    width - pad_x - options_width,
                    center(options_height),
                    options_width,
                    options_height,
                ),
            }
        }
    }

    /// The card's height in device pixels when it is `width` wide.
    pub fn height(&self, g: &Graphics, style: &Style, width: f32) -> f32 {
        self.arrange(g, style, width).height
    }

    /// Lays out and draws the card with its top-left corner at `(x, y)`.
    /// Returns its height.
    pub fn place(&self, painter: &mut Painter<'_, Id>, x: f32, y: f32, width: f32) -> f32 {
        let (g, style) = (painter.g, painter.style);
        let palette = &style.palette;
        let layout = self.arrange(g, style, width);
        let moved = |rect: Rect| Rect::new(rect.x + x, rect.y + y, rect.w, rect.h);
        if painter.paint {
            let frame = Rect::new(x, y, width, layout.height);
            let radius = style.px(CARD_RADIUS);
            g.fill_round_rect(palette.card, frame, radius);
            g.stroke_round_rect(palette.border, frame, radius);

            let tile = moved(layout.tile);
            g.fill_round_rect(palette.tile, tile, style.px(11.0));
            icon(g, style, self.icon, IconSize::Tile, palette.accent, tile);

            let text = moved(layout.text);
            let title = Rect::new(text.x, text.y, text.w, 0.0);
            g.text_wrapped(self.title, &style.fonts.title, palette.text, title);
            let description = Rect::new(
                text.x,
                text.y + layout.title_height + style.px(3.0),
                text.w - style.px(CARD_COLUMN_GAP),
                0.0,
            );
            g.text_wrapped(
                self.description,
                &style.fonts.body,
                palette.muted,
                description,
            );
        }
        let backdrop = std::mem::replace(&mut painter.backdrop, palette.card);
        self.options.place(painter, moved(layout.options));
        painter.backdrop = backdrop;
        layout.height
    }
}

/// Outlines a control that has the keyboard focus.
pub fn focus_ring(g: &Graphics, style: &Style, rect: Rect, radius: f32) {
    g.stroke_round_rect_wide(style.palette.accent, rect, radius, style.px(2.0).max(2.0));
}

fn label_text(g: &Graphics, style: &Style, text: &str, color: u32, x: f32, row: Rect) {
    let rect = Rect::new(x, row.y, 0.0, row.h);
    g.text(
        text,
        &style.fonts.body,
        color,
        rect,
        Align::Near,
        Align::Center,
    );
}

/// The square of a check box or the circle of a radio button within its row.
fn mark_rect(style: &Style, rect: Rect) -> Rect {
    let side = style.px(CHECK_SIDE);
    Rect::new(
        rect.x + style.px(1.0),
        rect.y + ((rect.h - side) / 2.0).round(),
        side,
        side,
    )
}

/// The fill of a box, circle or switch that is on.
fn primary_fill(style: &Style, visual: Visual) -> u32 {
    let palette = &style.palette;
    if visual.pressed {
        palette.primary_pressed
    } else if visual.hovered {
        palette.primary_hover
    } else {
        palette.primary
    }
}

/// The fill of a box, circle or switch that is off, if it has one.
fn idle_fill(style: &Style, visual: Visual) -> Option<u32> {
    let palette = &style.palette;
    if visual.pressed {
        Some(palette.field_pressed)
    } else if visual.hovered {
        Some(palette.field_hover)
    } else {
        None
    }
}

/// Draws a check box with its label.
pub fn checkbox(
    g: &Graphics,
    style: &Style,
    backdrop: u32,
    rect: Rect,
    label: &str,
    checked: bool,
    visual: Visual,
) {
    let palette = &style.palette;
    let tone = |color: u32| style.tone(color, backdrop, visual.enabled);
    let mark = mark_rect(style, rect);
    let radius = style.px(3.0);
    if checked {
        g.fill_round_rect(tone(primary_fill(style, visual)), mark, radius);
        g.stroke_round_rect(tone(palette.primary_edge), mark, radius);
        let unit = mark.w / CHECK_SIDE;
        let point = |x: f32, y: f32| (mark.x + x * unit, mark.y + y * unit);
        g.polyline(
            tone(palette.on_primary),
            &[point(4.4, 9.4), point(7.6, 12.6), point(13.8, 5.6)],
            (2.0 * unit).max(1.5),
        );
    } else {
        if let Some(fill) = idle_fill(style, visual) {
            g.fill_round_rect(fill, mark, radius);
        }
        g.stroke_round_rect(tone(palette.outline), mark, radius);
    }
    label_text(
        g,
        style,
        label,
        tone(palette.text),
        rect.x + style.px(CHECK_LABEL_OFFSET),
        rect,
    );
    if visual.focused {
        focus_ring(g, style, rect.inflate(style.px(4.0)), style.px(5.0));
    }
}

/// Draws a radio button with its label.
pub fn radio(
    g: &Graphics,
    style: &Style,
    backdrop: u32,
    rect: Rect,
    label: &str,
    selected: bool,
    visual: Visual,
) {
    let palette = &style.palette;
    let tone = |color: u32| style.tone(color, backdrop, visual.enabled);
    let mark = mark_rect(style, rect);
    if selected {
        g.fill_ellipse(tone(primary_fill(style, visual)), mark);
        g.stroke_ellipse(tone(palette.primary_edge), mark, style.hairline());
        let dot = style.px(8.0);
        let inset = (mark.w - dot) / 2.0;
        g.fill_ellipse(
            tone(palette.on_primary),
            Rect::new(mark.x + inset, mark.y + inset, dot, dot),
        );
    } else {
        if let Some(fill) = idle_fill(style, visual) {
            g.fill_ellipse(fill, mark);
        }
        g.stroke_ellipse(tone(palette.outline), mark, style.hairline());
    }
    label_text(
        g,
        style,
        label,
        tone(palette.text),
        rect.x + style.px(CHECK_LABEL_OFFSET),
        rect,
    );
    if visual.focused {
        focus_ring(g, style, rect.inflate(style.px(4.0)), style.px(5.0));
    }
}

/// Draws a switch, with its label after it when it has one.
pub fn toggle(
    g: &Graphics,
    style: &Style,
    backdrop: u32,
    rect: Rect,
    label: Option<&str>,
    on: bool,
    visual: Visual,
) {
    let palette = &style.palette;
    let tone = |color: u32| style.tone(color, backdrop, visual.enabled);
    let (width, height) = (style.px(TOGGLE_WIDTH), style.px(TOGGLE_HEIGHT));
    let track = Rect::new(
        rect.x,
        rect.y + ((rect.h - height) / 2.0).round(),
        width,
        height,
    );
    let radius = height / 2.0;
    // The knob grows a little under the pointer, as the system's switch does.
    let knob = style.px(if visual.hovered || visual.pressed {
        14.0
    } else {
        12.0
    });
    let margin = (height - knob) / 2.0;
    let (knob_x, knob_color) = if on {
        g.fill_round_rect(tone(primary_fill(style, visual)), track, radius);
        (track.right() - margin - knob, palette.on_primary)
    } else {
        if let Some(fill) = idle_fill(style, visual) {
            g.fill_round_rect(fill, track, radius);
        }
        g.stroke_round_rect(tone(palette.outline), track, radius);
        (track.x + margin, palette.muted)
    };
    g.fill_ellipse(
        tone(knob_color),
        Rect::new(knob_x, track.y + margin, knob, knob),
    );
    if let Some(label) = label {
        let x = track.right() + style.px(TOGGLE_LABEL_GAP);
        label_text(g, style, label, tone(palette.text), x, rect);
    }
    if visual.focused {
        focus_ring(g, style, rect.inflate(style.px(4.0)), style.px(5.0));
    }
}

/// Which way the spin strip at `(x, y)` of a numeric field steps the value:
/// up in its top half, down in its bottom half.
pub fn number_spin_at(style: &Style, rect: Rect, x: f32, y: f32) -> Option<i32> {
    let strip = rect.right() - style.px(NUMBER_SPIN_WIDTH);
    if !rect.contains(x, y) || x < strip {
        return None;
    }
    Some(if y < rect.y + rect.h / 2.0 { 1 } else { -1 })
}

/// The caret position nearest to `x` in a numeric field showing `text`.
pub fn number_caret_at(g: &Graphics, style: &Style, rect: Rect, text: &str, x: f32) -> usize {
    let origin = rect.x + style.px(NUMBER_TEXT_INSET);
    let mut nearest = (0, f32::MAX);
    for index in 0..=text.len() {
        if !text.is_char_boundary(index) {
            continue;
        }
        let edge = origin + g.measure(&text[..index], &style.fonts.body).0;
        let distance = (edge - x).abs();
        if distance < nearest.1 {
            nearest = (index, distance);
        }
    }
    nearest.0
}

/// Draws a numeric field. `caret` is present while the field is being edited.
pub fn number_field(
    g: &Graphics,
    style: &Style,
    backdrop: u32,
    rect: Rect,
    text: &str,
    caret: Option<Caret>,
    visual: Visual,
) {
    let palette = &style.palette;
    let tone = |color: u32| style.tone(color, backdrop, visual.enabled);
    let radius = style.px(4.0);
    let fill = if visual.hovered && caret.is_none() {
        palette.field_hover
    } else {
        palette.field
    };
    g.fill_round_rect(tone(fill), rect, radius);
    if caret.is_some() {
        g.stroke_round_rect_wide(palette.primary_edge, rect, radius, style.px(2.0).max(2.0));
    } else {
        g.stroke_round_rect(tone(palette.outline), rect, radius);
    }

    let font = &style.fonts.body;
    let text_x = rect.x + style.px(NUMBER_TEXT_INSET);
    let (text_width, text_height) = g.measure(text, font);
    let line_height = text_height.max(style.line_height(g, font));
    let line_top = rect.y + ((rect.h - line_height) / 2.0).round();
    let selected = caret.is_some_and(|caret| caret.all_selected) && !text.is_empty();
    if selected {
        let highlight = Rect::new(
            text_x - style.px(1.0),
            line_top,
            text_width + style.px(2.0),
            line_height,
        );
        g.fill_rect(palette.primary, highlight);
    }
    let ink = if selected {
        palette.on_primary
    } else {
        tone(palette.text)
    };
    label_text(g, style, text, ink, text_x, rect);
    if let Some(caret) = caret.filter(|caret| caret.visible && !selected) {
        let before = text.get(..caret.index).unwrap_or(text);
        let x = text_x + g.measure(before, font).0;
        g.fill_rect(
            palette.text,
            Rect::new(x, line_top, style.hairline(), line_height),
        );
    }

    // The steppers are only offered while the field can be changed.
    if visual.enabled {
        let center_x = rect.right() - style.px(NUMBER_SPIN_WIDTH / 2.0);
        let center_y = rect.y + rect.h / 2.0;
        let (half, rise) = (style.scale * 3.5, style.scale * 3.5);
        let gap = style.scale * 2.5;
        let stroke = (style.scale * 1.2).max(1.0);
        for direction in [-1.0, 1.0] {
            let base = center_y + direction * gap;
            let tip = base + direction * rise;
            g.polyline(
                palette.muted,
                &[
                    (center_x - half, base),
                    (center_x, tip),
                    (center_x + half, base),
                ],
                stroke,
            );
        }
    }
}

/// Draws the closed part of a dropdown.
pub fn dropdown_field(
    g: &Graphics,
    style: &Style,
    backdrop: u32,
    rect: Rect,
    text: &str,
    open: bool,
    visual: Visual,
) {
    let palette = &style.palette;
    let tone = |color: u32| style.tone(color, backdrop, visual.enabled);
    let radius = style.px(5.0);
    let fill = if visual.pressed {
        palette.field_pressed
    } else if visual.hovered {
        palette.field_hover
    } else {
        palette.field
    };
    g.fill_round_rect(tone(fill), rect, radius);
    let edge = if open {
        palette.primary_edge
    } else {
        palette.outline
    };
    g.stroke_round_rect(tone(edge), rect, radius);
    label_text(
        g,
        style,
        text,
        tone(palette.text),
        rect.x + style.px(DROPDOWN_TEXT_INSET),
        rect,
    );
    let center_x = rect.right() - style.px(18.0);
    let center_y = rect.y + rect.h / 2.0;
    let (half, drop) = (style.scale * 4.0, style.scale * 2.0);
    g.polyline(
        tone(palette.text),
        &[
            (center_x - half, center_y - drop),
            (center_x, center_y + drop),
            (center_x + half, center_y - drop),
        ],
        (style.scale * 1.2).max(1.0),
    );
    if visual.focused {
        focus_ring(g, style, rect.inflate(style.px(3.0)), style.px(7.0));
    }
}

/// Where a dropdown's list of `count` items opens: under `field`, or above
/// it when there is no room left under it inside `window`.
pub fn dropdown_list_rect(style: &Style, field: Rect, count: usize, window: Rect) -> Rect {
    let gap = style.px(3.0);
    let height = style.px(DROPDOWN_ITEM_HEIGHT) * count as f32 + 2.0 * style.px(3.0);
    let below = field.bottom() + gap;
    let y = if below + height <= window.bottom() || field.y - gap - height < window.y {
        below
    } else {
        field.y - gap - height
    };
    Rect::new(field.x, y, field.w, height)
}

/// The rectangle of one item inside a dropdown's list.
pub fn dropdown_item_rect(style: &Style, list: Rect, index: usize) -> Rect {
    let inset = style.px(3.0);
    let height = style.px(DROPDOWN_ITEM_HEIGHT);
    Rect::new(
        list.x + inset,
        list.y + inset + height * index as f32,
        list.w - 2.0 * inset,
        height,
    )
}

/// Draws an open dropdown's list. `highlighted` is the item the pointer or
/// the arrow keys are on.
pub fn dropdown_list(
    g: &Graphics,
    style: &Style,
    list: Rect,
    items: &[&str],
    selected: usize,
    highlighted: Option<usize>,
) {
    let palette = &style.palette;
    let radius = style.px(6.0);
    g.fill_round_rect(palette.card, list, radius);
    g.stroke_round_rect(palette.outline, list, radius);
    for (index, item) in items.iter().enumerate() {
        let rect = dropdown_item_rect(style, list, index);
        let fill = if index == selected {
            Some(palette.selected)
        } else if highlighted == Some(index) {
            Some(palette.hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            g.fill_round_rect(fill, rect, style.px(4.0));
        }
        if highlighted == Some(index) && index == selected {
            g.stroke_round_rect(palette.primary_edge, rect, style.px(4.0));
        }
        let x = rect.x + style.px(DROPDOWN_TEXT_INSET) - style.px(3.0);
        label_text(g, style, item, palette.text, x, rect);
    }
}

/// The rectangles of `count` equal segments inside a segmented choice.
pub fn segment_rects(style: &Style, frame: Rect, count: usize) -> Vec<Rect> {
    let inset = style.px(SEGMENT_INSET) + style.hairline();
    let inner = frame.inflate(-inset);
    let width = inner.w / count.max(1) as f32;
    (0..count)
        .map(|index| {
            let left = (inner.x + width * index as f32).round();
            let right = (inner.x + width * (index + 1) as f32).round();
            Rect::new(left, inner.y, right - left, inner.h)
        })
        .collect()
}

/// Draws a segmented choice. `visuals` has one entry per label.
pub fn segments(
    g: &Graphics,
    style: &Style,
    frame: Rect,
    labels: &[&str],
    selected: usize,
    visuals: &[Visual],
) {
    let palette = &style.palette;
    g.stroke_round_rect(palette.outline, frame, style.px(10.0));
    let rects = segment_rects(style, frame, labels.len());
    let radius = style.px(8.0);
    for (index, (label, rect)) in labels.iter().zip(rects).enumerate() {
        let visual = visuals.get(index).copied().unwrap_or_default();
        let chosen = index == selected;
        if chosen {
            g.fill_round_rect(primary_fill(style, visual), rect, radius);
        } else if let Some(fill) = idle_fill(style, visual) {
            g.fill_round_rect(fill, rect, radius);
        }
        let (font, color) = if chosen {
            (&style.fonts.strong, palette.on_primary)
        } else {
            (&style.fonts.body, palette.text)
        };
        g.text(label, font, color, rect, Align::Center, Align::Center);
        if visual.focused {
            // Inside the segment, where it shows against the fill.
            let ring = rect.inflate(-style.px(2.0));
            let color = if chosen {
                palette.on_primary
            } else {
                palette.accent
            };
            g.stroke_round_rect_wide(color, ring, style.px(6.0), style.px(2.0).max(2.0));
        }
    }
}

/// Draws a button showing a color to change.
pub fn swatch(g: &Graphics, style: &Style, backdrop: u32, rect: Rect, color: u32, visual: Visual) {
    let palette = &style.palette;
    let tone = |color: u32| style.tone(color, backdrop, visual.enabled);
    let radius = style.px(5.0);
    g.fill_round_rect(tone(color | 0xFF00_0000), rect, radius);
    let edge = if visual.hovered || visual.pressed {
        palette.primary_edge
    } else {
        palette.outline
    };
    g.stroke_round_rect(tone(edge), rect, radius);
    if visual.pressed {
        g.stroke_round_rect(
            palette.primary_edge,
            rect.inflate(-style.hairline()),
            radius,
        );
    }
    if visual.focused {
        focus_ring(g, style, rect.inflate(style.px(3.0)), style.px(7.0));
    }
}

/// Draws a push button; `primary` marks the window's default action.
pub fn button(
    g: &Graphics,
    style: &Style,
    backdrop: u32,
    rect: Rect,
    label: &str,
    primary: bool,
    visual: Visual,
) {
    let palette = &style.palette;
    let tone = |color: u32| style.tone(color, backdrop, visual.enabled);
    let radius = style.px(5.0);
    let (fill, edge, ink) = if primary {
        (
            primary_fill(style, visual),
            palette.primary_edge,
            palette.on_primary,
        )
    } else {
        let fill = if visual.pressed {
            palette.field_pressed
        } else if visual.hovered {
            palette.field_hover
        } else {
            palette.field
        };
        let edge = if visual.hovered || visual.pressed {
            palette.primary_edge
        } else {
            palette.border
        };
        (fill, edge, palette.text)
    };
    g.fill_round_rect(tone(fill), rect, radius);
    g.stroke_round_rect(tone(edge), rect, radius);
    g.text(
        label,
        &style.fonts.body,
        tone(ink),
        rect,
        Align::Center,
        Align::Center,
    );
    if visual.focused {
        focus_ring(g, style, rect.inflate(style.px(3.0)), style.px(7.0));
    }
}

/// Draws a scroll bar's thumb.
pub fn scrollbar_thumb(g: &Graphics, style: &Style, thumb: Rect, visual: Visual) {
    let palette = &style.palette;
    let color = if visual.pressed {
        palette.muted
    } else if visual.hovered {
        palette.outline
    } else {
        palette.border
    };
    g.fill_round_rect(color, thumb, thumb.w / 2.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::tests::Canvas;
    use crate::gfx::GdiPlus;

    fn target(id: u32, bounds: Rect) -> Target<u32> {
        Target {
            id,
            bounds,
            visible: Some(bounds),
            enabled: true,
            tab_stop: true,
            cursor: Cursor::Hand,
        }
    }

    /// Three controls in a row, 100 pixels apart; the second is disabled and
    /// the third is not a tab stop.
    fn sample() -> Interaction<u32> {
        let mut interaction = Interaction::default();
        interaction.begin_layout();
        interaction.add(target(1, Rect::new(0.0, 0.0, 50.0, 20.0)));
        interaction.add(Target {
            enabled: false,
            ..target(2, Rect::new(100.0, 0.0, 50.0, 20.0))
        });
        interaction.add(Target {
            tab_stop: false,
            cursor: Cursor::Text,
            ..target(3, Rect::new(200.0, 0.0, 50.0, 20.0))
        });
        interaction.add(target(4, Rect::new(300.0, 0.0, 50.0, 20.0)));
        interaction.end_layout();
        interaction
    }

    #[test]
    fn colors_blend_by_channel() {
        assert_eq!(mix(0xFF00_0000, 0xFFFF_FFFF, 0.0), 0xFF00_0000);
        assert_eq!(mix(0xFF00_0000, 0xFFFF_FFFF, 1.0), 0xFFFF_FFFF);
        assert_eq!(mix(0xFF00_0000, 0xFFFF_FFFF, 0.5), 0xFF80_8080);
        assert_eq!(mix(0xFF10_2030, 0xFF30_2010, 0.5), 0xFF20_2020);
        assert_eq!(mix(0xFF00_0000, 0xFFFF_FFFF, 7.0), 0xFFFF_FFFF);
    }

    #[test]
    fn palettes_derive_their_shades_from_the_popup_theme() {
        for dark in [true, false] {
            let (palette, theme) = (Palette::new(dark), Theme::new(dark));
            assert_eq!(palette.surface, theme.surface);
            assert_eq!(palette.card, theme.card);
            assert_eq!(palette.text, theme.text);
            assert_eq!(palette.accent, theme.accent);
            // Derived shades stay opaque and distinct from what they sit on.
            for shade in [
                palette.field,
                palette.selected,
                palette.tile,
                palette.outline,
            ] {
                assert_eq!(shade >> 24, 0xFF);
                assert_ne!(shade, palette.card);
            }
        }
    }

    #[test]
    fn the_topmost_enabled_control_is_hit() {
        let mut interaction = sample();
        assert_eq!(interaction.target_at(10.0, 10.0).map(|hit| hit.id), Some(1));
        assert_eq!(interaction.target_at(60.0, 10.0).map(|hit| hit.id), None);
        assert_eq!(interaction.target_at(10.0, 20.0).map(|hit| hit.id), None);
        // Disabled controls are not hit, and do not change the cursor.
        assert_eq!(interaction.target_at(110.0, 10.0).map(|hit| hit.id), None);
        assert_eq!(interaction.cursor_at(110.0, 10.0), Cursor::Arrow);
        assert_eq!(interaction.cursor_at(210.0, 10.0), Cursor::Text);
        assert_eq!(interaction.cursor_at(10.0, 10.0), Cursor::Hand);

        // A control added later covers the ones under it.
        interaction.add(target(9, Rect::new(0.0, 0.0, 400.0, 20.0)));
        assert_eq!(interaction.target_at(10.0, 10.0).map(|hit| hit.id), Some(9));
        assert_eq!(
            interaction.target_at(110.0, 10.0).map(|hit| hit.id),
            Some(9)
        );
    }

    #[test]
    fn controls_scrolled_out_of_view_cannot_be_clicked() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(10, 10);
        let g = canvas.graphics();
        let style = Style::new(1.0, true).unwrap();
        let mut interaction = Interaction::default();
        let mut painter = Painter {
            g: &g,
            style: &style,
            interaction: &mut interaction,
            paint: false,
            clip: Some(Rect::new(0.0, 100.0, 300.0, 100.0)),
            backdrop: 0xFF00_0000,
        };
        painter.target(
            1u32,
            Rect::new(0.0, 90.0, 50.0, 20.0),
            true,
            true,
            Cursor::Hand,
        );
        painter.target(
            2u32,
            Rect::new(0.0, 300.0, 50.0, 20.0),
            true,
            true,
            Cursor::Hand,
        );

        // Only the part inside the visible area can be hit.
        assert_eq!(interaction.target_at(10.0, 95.0).map(|hit| hit.id), None);
        assert_eq!(
            interaction.target_at(10.0, 105.0).map(|hit| hit.id),
            Some(1)
        );
        assert_eq!(interaction.target_at(10.0, 305.0).map(|hit| hit.id), None);
        // A hidden control keeps its place in the focus order and its bounds,
        // so focusing it can scroll it into view.
        assert_eq!(interaction.focus_next(false), Some(1));
        assert_eq!(interaction.focus_next(false), Some(2));
        assert_eq!(
            interaction.target(2).map(|hit| hit.bounds),
            Some(Rect::new(0.0, 300.0, 50.0, 20.0))
        );
    }

    #[test]
    fn tab_moves_through_enabled_tab_stops_and_wraps() {
        let mut interaction = sample();
        assert_eq!(interaction.focused(), None);
        assert_eq!(interaction.focus_next(false), Some(1));
        assert_eq!(interaction.focus_next(false), Some(4));
        assert_eq!(interaction.focus_next(false), Some(1));
        assert_eq!(interaction.focus_next(true), Some(4));
        assert_eq!(interaction.focus_next(true), Some(1));

        // Without a focus, Shift+Tab starts from the end.
        interaction.set_focus(None, false);
        assert_eq!(interaction.focus_next(true), Some(4));
        // From a control that is not a tab stop, Tab starts from the beginning.
        interaction.set_focus(Some(3), false);
        assert_eq!(interaction.focus_next(false), Some(1));

        let mut empty: Interaction<u32> = Interaction::default();
        assert_eq!(empty.focus_next(false), None);
    }

    #[test]
    fn the_focus_outline_follows_the_keyboard_not_the_mouse() {
        let mut interaction = sample();
        interaction.pointer_pressed(10.0, 10.0);
        interaction.pointer_released(10.0, 10.0);
        assert_eq!(interaction.focused(), Some(1));
        assert!(!interaction.visual(1, true).focused);

        interaction.show_focus();
        assert!(interaction.visual(1, true).focused);
        interaction.focus_next(false);
        assert!(interaction.visual(4, true).focused);
        assert!(!interaction.visual(1, true).focused);

        interaction.pointer_pressed(10.0, 10.0);
        assert!(!interaction.visual(1, true).focused);
    }

    #[test]
    fn a_click_needs_press_and_release_on_the_same_control() {
        let mut interaction = sample();
        assert_eq!(interaction.pointer_pressed(10.0, 10.0), Some(1));
        assert!(interaction.visual(1, true).pressed);
        assert_eq!(interaction.pointer_released(12.0, 12.0), Some(1));
        assert!(!interaction.visual(1, true).pressed);

        // Dragging off the control cancels the click and does not make the
        // control under the pointer look hovered meanwhile.
        assert_eq!(interaction.pointer_pressed(10.0, 10.0), Some(1));
        assert!(interaction.pointer_moved(Some((310.0, 10.0))));
        assert!(!interaction.visual(1, true).pressed);
        assert!(!interaction.visual(4, true).hovered);
        assert_eq!(interaction.pointer_released(310.0, 10.0), None);
        assert!(interaction.visual(4, true).hovered);

        // Pressing empty space or a disabled control starts nothing and
        // leaves the focus alone.
        assert_eq!(interaction.pointer_pressed(60.0, 10.0), None);
        assert_eq!(interaction.pointer_pressed(110.0, 10.0), None);
        assert_eq!(interaction.focused(), Some(1));

        interaction.pointer_pressed(10.0, 10.0);
        interaction.cancel_press();
        assert_eq!(interaction.pointer_released(10.0, 10.0), None);
    }

    #[test]
    fn hover_follows_the_pointer_and_reports_changes() {
        let mut interaction = sample();
        assert!(interaction.pointer_moved(Some((10.0, 10.0))));
        assert!(!interaction.pointer_moved(Some((12.0, 10.0))));
        assert!(interaction.visual(1, true).hovered);
        assert!(!interaction.visual(1, false).hovered);
        assert!(interaction.pointer_moved(Some((310.0, 10.0))));
        assert_eq!(interaction.hovered(), Some(4));
        assert!(interaction.pointer_moved(None));
        assert_eq!(interaction.hovered(), None);
    }

    #[test]
    fn state_for_controls_that_disappear_is_dropped() {
        let mut interaction = sample();
        interaction.focus_next(true);
        interaction.pointer_pressed(310.0, 10.0);
        assert_eq!(
            (interaction.focused(), interaction.hovered()),
            (Some(4), Some(4))
        );
        assert!(interaction.visual(4, true).pressed);

        // The next layout no longer has control 4, and disables control 1.
        interaction.begin_layout();
        interaction.add(Target {
            enabled: false,
            ..target(1, Rect::new(0.0, 0.0, 50.0, 20.0))
        });
        interaction.end_layout();
        assert_eq!((interaction.focused(), interaction.hovered()), (None, None));
        assert_eq!(interaction.focus_next(false), None);
        // The interrupted press does not turn into a click on what is there now.
        assert_eq!(interaction.pointer_released(310.0, 10.0), None);
    }

    #[test]
    fn typing_replaces_a_selection_and_accepts_only_digits() {
        let mut edit = NumberEdit::new(25, true);
        assert!(edit.all_selected());
        assert!(!edit.insert('x', false));
        assert!(!edit.insert('-', false));
        assert!(!edit.insert(' ', false));
        assert_eq!(edit.text(), "25");
        assert!(edit.insert('4', false));
        assert!(edit.insert('0', false));
        assert_eq!(
            (edit.text(), edit.caret(), edit.all_selected()),
            ("40", 2, false)
        );

        edit.move_caret(CaretMove::Left);
        assert!(edit.insert('5', false));
        assert_eq!((edit.text(), edit.caret()), ("450", 2));
        edit.backspace();
        assert_eq!((edit.text(), edit.caret()), ("40", 1));
        edit.delete();
        assert_eq!((edit.text(), edit.caret()), ("4", 1));
        edit.delete();
        edit.move_caret(CaretMove::Home);
        edit.backspace();
        assert_eq!(edit.text(), "4");

        edit.select_all();
        edit.backspace();
        assert_eq!((edit.text(), edit.caret()), ("", 0));
        edit.select_all();
        assert!(!edit.all_selected());

        // The length is capped so the text always fits a number.
        for _ in 0..20 {
            edit.insert('9', false);
        }
        assert_eq!(edit.text(), "999999999");
        assert_eq!(edit.value(0, 7_200), Some(7_200));
    }

    #[test]
    fn carets_move_within_the_text() {
        let mut edit = NumberEdit::new(300, true);
        edit.move_caret(CaretMove::Left);
        assert_eq!((edit.caret(), edit.all_selected()), (0, false));
        edit.select_all();
        edit.move_caret(CaretMove::Right);
        assert_eq!(edit.caret(), 3);
        edit.move_caret(CaretMove::Right);
        assert_eq!(edit.caret(), 3);
        edit.move_caret(CaretMove::Home);
        edit.move_caret(CaretMove::Left);
        assert_eq!(edit.caret(), 0);
        edit.move_caret(CaretMove::End);
        assert_eq!(edit.caret(), 3);
        edit.set_caret(1);
        assert_eq!(edit.caret(), 1);
        edit.set_caret(99);
        assert_eq!(edit.caret(), 3);
    }

    #[test]
    fn typed_numbers_are_clamped_and_blank_text_is_no_number() {
        let typed = |text: &str| {
            let mut edit = NumberEdit::new(0, true);
            for character in text.chars() {
                edit.insert(character, true);
            }
            edit
        };
        assert_eq!(typed("18").value(1, 99), Some(18));
        assert_eq!(typed("0").value(1, 99), Some(1));
        assert_eq!(typed("500").value(1, 99), Some(99));
        assert_eq!(typed("007").value(1, 99), Some(7));
        assert_eq!(typed("-5").value(-10, 10), Some(-5));
        assert_eq!(typed("-50").value(-10, 10), Some(-10));
        assert_eq!(typed("-").value(-10, 10), None);
        assert_eq!(NumberEdit::new(5, true).value(1, 9), Some(5));
        let mut blank = NumberEdit::new(5, true);
        blank.backspace();
        assert_eq!(blank.value(1, 9), None);

        // A minus sign is only accepted in front, and only once.
        let mut signed = typed("-5");
        assert!(!signed.insert('-', true));
        signed.move_caret(CaretMove::Home);
        assert!(!signed.insert('-', true));
        assert!(!signed.insert('3', true));
        assert_eq!(signed.text(), "-5");
    }

    #[test]
    fn uncommitted_input_is_text_that_does_not_say_the_value() {
        let mut edit = NumberEdit::new(15, true);
        assert!(!edit.differs_from(15));
        edit.insert('1', false);
        assert!(edit.differs_from(15));
        edit.insert('5', false);
        assert!(!edit.differs_from(15));
        // Leading zeros still say the same number; an out-of-range number
        // differs even though committing it would clamp it.
        edit.move_caret(CaretMove::Home);
        edit.insert('0', false);
        assert_eq!(edit.text(), "015");
        assert!(!edit.differs_from(15));
        edit.select_all();
        edit.backspace();
        assert!(edit.differs_from(15));
    }

    #[test]
    fn scrolling_stays_within_the_content() {
        let mut scroll = Scroll::default();
        scroll.set_extent(300.0, 400.0);
        assert!(!scroll.is_scrollable());
        assert!(!scroll.scroll_by(50.0));
        assert_eq!(scroll.thumb(Rect::new(0.0, 0.0, 6.0, 400.0), 24.0), None);

        scroll.set_extent(1_000.0, 400.0);
        assert_eq!(scroll.max_offset(), 600.0);
        assert!(scroll.scroll_by(150.0));
        assert!(scroll.scroll_by(-500.0));
        assert_eq!(scroll.offset(), 0.0);
        assert!(!scroll.scroll_by(-10.0));
        assert!(scroll.scroll_by(5_000.0));
        assert_eq!(scroll.offset(), 600.0);

        // Shrinking the content pulls the offset back into range.
        scroll.set_extent(700.0, 400.0);
        assert_eq!(scroll.offset(), 300.0);
        scroll.set_extent(100.0, 400.0);
        assert_eq!(scroll.offset(), 0.0);
    }

    #[test]
    fn revealing_scrolls_only_as_far_as_needed() {
        let mut scroll = Scroll::default();
        scroll.set_extent(1_000.0, 400.0);
        assert!(!scroll.reveal(100.0, 200.0));
        assert!(scroll.reveal(500.0, 560.0));
        assert_eq!(scroll.offset(), 160.0);
        assert!(scroll.reveal(100.0, 150.0));
        assert_eq!(scroll.offset(), 100.0);
        // Something taller than the viewport is shown from its top.
        assert!(scroll.reveal(300.0, 900.0));
        assert_eq!(scroll.offset(), 300.0);
        assert!(scroll.reveal(950.0, 1_000.0));
        assert_eq!(scroll.offset(), 600.0);
    }

    #[test]
    fn the_scroll_thumb_maps_to_offsets_and_back() {
        let track = Rect::new(0.0, 100.0, 6.0, 400.0);
        let mut scroll = Scroll::default();
        scroll.set_extent(1_000.0, 400.0);
        assert_eq!(
            scroll.thumb(track, 24.0),
            Some(Rect::new(0.0, 100.0, 6.0, 160.0))
        );
        scroll.scroll_to(600.0);
        assert_eq!(
            scroll.thumb(track, 24.0),
            Some(Rect::new(0.0, 340.0, 6.0, 160.0))
        );
        scroll.scroll_to(300.0);
        assert_eq!(
            scroll.thumb(track, 24.0),
            Some(Rect::new(0.0, 220.0, 6.0, 160.0))
        );

        assert_eq!(scroll.offset_for_thumb(track, 24.0, 100.0), 0.0);
        assert_eq!(scroll.offset_for_thumb(track, 24.0, 220.0), 300.0);
        assert_eq!(scroll.offset_for_thumb(track, 24.0, 340.0), 600.0);
        assert_eq!(scroll.offset_for_thumb(track, 24.0, 9_000.0), 600.0);
        assert_eq!(scroll.offset_for_thumb(track, 24.0, -50.0), 0.0);

        // Very long content keeps a thumb large enough to grab.
        scroll.set_extent(100_000.0, 400.0);
        assert_eq!(scroll.thumb(track, 24.0).map(|thumb| thumb.h), Some(24.0));
    }

    #[test]
    fn dropdown_lists_open_upward_when_there_is_no_room_below() {
        let _gdiplus = GdiPlus::start().unwrap();
        let style = Style::new(1.0, true).unwrap();
        let window = Rect::new(0.0, 0.0, 800.0, 600.0);
        let high = Rect::new(100.0, 100.0, 240.0, 39.0);
        let list = dropdown_list_rect(&style, high, 3, window);
        assert_eq!(list, Rect::new(100.0, 142.0, 240.0, 117.0));
        assert_eq!(
            dropdown_item_rect(&style, list, 1),
            Rect::new(103.0, 182.0, 234.0, 37.0)
        );

        let low = Rect::new(100.0, 540.0, 240.0, 39.0);
        assert_eq!(dropdown_list_rect(&style, low, 3, window).bottom(), 537.0);
        // With no room either way the list still opens downward.
        let short = Rect::new(0.0, 0.0, 800.0, 150.0);
        let middle = Rect::new(100.0, 60.0, 240.0, 39.0);
        assert_eq!(dropdown_list_rect(&style, middle, 3, short).y, 102.0);
    }

    #[test]
    fn spin_strips_and_carets_are_found_by_position() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(10, 10);
        let g = canvas.graphics();
        let style = Style::new(1.0, true).unwrap();
        let field = Rect::new(100.0, 50.0, 96.0, 35.0);
        assert_eq!(number_spin_at(&style, field, 110.0, 60.0), None);
        assert_eq!(number_spin_at(&style, field, 185.0, 55.0), Some(1));
        assert_eq!(number_spin_at(&style, field, 185.0, 80.0), Some(-1));
        assert_eq!(number_spin_at(&style, field, 185.0, 90.0), None);

        assert_eq!(number_caret_at(&g, &style, field, "3600", 0.0), 0);
        assert_eq!(number_caret_at(&g, &style, field, "3600", 111.0), 0);
        assert_eq!(number_caret_at(&g, &style, field, "3600", 500.0), 4);
        let one = g.measure("3", &style.fonts.body).0;
        assert_eq!(
            number_caret_at(&g, &style, field, "3600", 111.0 + one + 1.0),
            1
        );
        assert_eq!(number_caret_at(&g, &style, field, "", 150.0), 0);
    }

    #[test]
    fn rows_and_columns_arrange_their_children() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(400, 200);
        let g = canvas.graphics();
        let style = Style::new(1.0, true).unwrap();
        let number = |id: u32| Node::Number {
            id,
            text: "5".to_string(),
            caret: None,
            enabled: true,
        };
        let swatch = |id: u32| Node::Swatch {
            id,
            color: 0xFF12_3456,
            enabled: id != 4,
        };
        let row = Node::Row {
            gap: 10.0,
            children: vec![number(1), swatch(2)],
        };
        assert_eq!(row.measure(&g, &style), (96.0 + 10.0 + 38.0, 35.0));
        let column = Node::Column {
            gap: 12.0,
            align: HAlign::Start,
            children: vec![row, swatch(3), Node::Indent(18.0, Box::new(swatch(4)))],
        };
        assert_eq!(
            column.measure(&g, &style),
            (144.0, 35.0 + 12.0 + 32.0 + 12.0 + 32.0)
        );

        let mut interaction = Interaction::default();
        interaction.begin_layout();
        let mut painter = Painter {
            g: &g,
            style: &style,
            interaction: &mut interaction,
            paint: true,
            clip: None,
            backdrop: style.palette.card,
        };
        column.place(&mut painter, Rect::new(20.0, 10.0, 144.0, 123.0));
        interaction.end_layout();
        let bounds = |id: u32| interaction.target(id).map(|target| target.bounds);
        assert_eq!(bounds(1), Some(Rect::new(20.0, 10.0, 96.0, 35.0)));
        // The shorter swatch is centered beside the taller field.
        assert_eq!(bounds(2), Some(Rect::new(126.0, 12.0, 38.0, 32.0)));
        // Children of a column start at its left edge, or after their indent.
        assert_eq!(bounds(3), Some(Rect::new(20.0, 57.0, 38.0, 32.0)));
        assert_eq!(bounds(4), Some(Rect::new(38.0, 101.0, 38.0, 32.0)));
        // Tab order is the order the controls were laid out in; the disabled
        // one is skipped.
        assert_eq!(interaction.focus_next(false), Some(1));
        assert_eq!(interaction.focus_next(false), Some(2));
        assert_eq!(interaction.focus_next(false), Some(3));
        assert_eq!(interaction.focus_next(false), Some(1));
    }

    #[test]
    fn spreads_and_segments_fill_the_width_they_are_given() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(500, 100);
        let g = canvas.graphics();
        let style = Style::new(1.0, true).unwrap();
        let mut interaction = Interaction::default();
        interaction.begin_layout();
        let mut painter = Painter {
            g: &g,
            style: &style,
            interaction: &mut interaction,
            paint: true,
            clip: None,
            backdrop: style.palette.card,
        };
        let spread: Node<u32> = Node::Spread(
            Box::new(Node::label("Show projected usage")),
            Box::new(Node::Toggle {
                id: 1,
                label: None,
                on: true,
                enabled: true,
            }),
        );
        let (natural, height) = spread.measure(&g, &style);
        assert!(natural < 400.0);
        spread.place(&mut painter, Rect::new(10.0, 10.0, 400.0, height));
        let segments: Node<u32> = Node::Segments {
            options: vec![(2, "Learned routine"), (3, "Recent pace")],
            selected: 1,
        };
        let (_, segments_height) = segments.measure(&g, &style);
        segments.place(&mut painter, Rect::new(10.0, 50.0, 400.0, segments_height));
        interaction.end_layout();

        let bounds = |id: u32| interaction.target(id).map(|target| target.bounds).unwrap();
        assert_eq!(bounds(1).right(), 410.0);
        assert_eq!(bounds(1).w, 40.0);
        // Two equal segments inside the frame's border and padding.
        assert_eq!(
            bounds(2),
            Rect::new(14.0, 54.0, 196.0, segments_height - 8.0)
        );
        assert_eq!(
            bounds(3),
            Rect::new(210.0, 54.0, 196.0, segments_height - 8.0)
        );
        // Only the selected segment is a tab stop.
        assert_eq!(interaction.focus_next(false), Some(1));
        assert_eq!(interaction.focus_next(false), Some(3));
        assert_eq!(interaction.focus_next(false), Some(1));
    }

    #[test]
    fn long_text_wraps_at_its_limit() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(10, 10);
        let g = canvas.graphics();
        let style = Style::new(1.0, true).unwrap();
        let text = |wrap: Option<f32>| -> Node<u32> {
            Node::Text {
                text: "Low contrast 1.07:1. Try #000000.".to_string(),
                tone: Tone::Warning,
                wrap,
            }
        };
        let (single_width, single_height) = text(None).measure(&g, &style);
        assert!(single_width > 150.0);
        let (wrapped_width, wrapped_height) = text(Some(150.0)).measure(&g, &style);
        assert_eq!(wrapped_width, 150.0);
        assert!(wrapped_height >= 2.0 * single_height - 2.0);
        // Text shorter than the limit keeps its natural size.
        assert_eq!(
            text(Some(900.0)).measure(&g, &style),
            (single_width, single_height)
        );
    }

    #[test]
    fn cards_stack_their_options_when_the_text_would_be_squeezed() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(900, 400);
        let g = canvas.graphics();
        let style = Style::new(1.0, true).unwrap();
        let card = |options: Node<'static, u32>| Card {
            icon: Icon::Bars,
            title: "Usage threshold alerts",
            description: "Get alerted when usage reaches these levels.",
            min_height: 108.0,
            split: Split::Share {
                text: 1.1,
                options: 1.4,
            },
            fill: None,
            options,
        };
        let options = || Node::Dropdown {
            id: 1u32,
            text: "Learn automatically",
            width: 310.0,
            open: false,
            enabled: true,
        };
        let mut interaction = Interaction::default();
        let mut place = |width: f32| {
            interaction.begin_layout();
            let mut painter = Painter {
                g: &g,
                style: &style,
                interaction: &mut interaction,
                paint: true,
                clip: None,
                backdrop: style.palette.surface,
            };
            let height = card(options()).place(&mut painter, 10.0, 20.0, width);
            assert_eq!(height, card(options()).height(&g, &style, width));
            (
                height,
                interaction.target(1).map(|target| target.bounds).unwrap(),
            )
        };

        // Wide: the minimum height applies and the options sit at the right
        // edge, centered vertically.
        let (height, bounds) = place(880.0);
        assert_eq!(height, 108.0);
        assert_eq!(
            bounds,
            Rect::new(10.0 + 880.0 - 25.0 - 310.0, 20.0 + 35.0, 310.0, 39.0)
        );

        // Narrow: the options move under the text, lined up with it, and the
        // card grows to hold them.
        let (height, bounds) = place(520.0);
        assert!(height > 108.0);
        assert_eq!(bounds.x, 10.0 + 25.0 + 68.0);
        assert!(bounds.bottom() <= 20.0 + height - 13.0);
        assert!(bounds.y > 20.0 + 48.0);
    }
}
