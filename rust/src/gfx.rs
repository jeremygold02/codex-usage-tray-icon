//! Minimal safe wrappers over the GDI+ flat API used by the tray icon and popup.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::Graphics::GdiPlus::{
    CombineModeReplace, DashStyleDash, DashStyleDot, FillModeAlternate, GdipAddPathArc,
    GdipAddPathString, GdipCloneStringFormat, GdipClosePathFigure, GdipCreateBitmapFromScan0,
    GdipCreateFont, GdipCreateFontFamilyFromName, GdipCreateFromHDC, GdipCreateHICONFromBitmap,
    GdipCreateMatrix2, GdipCreatePath, GdipCreatePen1, GdipCreateSolidFill, GdipDeleteBrush,
    GdipDeleteFont, GdipDeleteFontFamily, GdipDeleteGraphics, GdipDeleteMatrix, GdipDeletePath,
    GdipDeletePen, GdipDeleteStringFormat, GdipDisposeImage, GdipDrawArc, GdipDrawEllipse,
    GdipDrawImageRect, GdipDrawLine, GdipDrawLines, GdipDrawPath, GdipDrawString, GdipFillEllipse,
    GdipFillPath, GdipFillPolygon, GdipFillRectangle, GdipGetImageGraphicsContext,
    GdipGetPathWorldBounds, GdipGraphicsClear, GdipMeasureString, GdipResetClip, GdipSetClipRect,
    GdipSetInterpolationMode, GdipSetPenDashStyle, GdipSetPenEndCap, GdipSetPenLineJoin,
    GdipSetPenStartCap, GdipSetPixelOffsetMode, GdipSetSmoothingMode, GdipSetStringFormatAlign,
    GdipSetStringFormatFlags, GdipSetStringFormatLineAlign, GdipSetTextRenderingHint,
    GdipStringFormatGetGenericTypographic, GdipTransformPath, GdiplusShutdown, GdiplusStartup,
    GdiplusStartupInput, GpBitmap, GpBrush, GpFont, GpFontFamily, GpGraphics, GpImage, GpPath,
    GpSolidFill, GpStringFormat, InterpolationModeHighQualityBicubic, LineCapRound, LineJoinRound,
    PixelOffsetModeHalf, PointF, RectF, SmoothingModeAntiAlias, TextRenderingHintClearTypeGridFit,
    UnitPixel, UnitWorld,
};
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;

use crate::win::wide;

const STATUS_OK: i32 = 0;
const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;
const FONT_STYLE_REGULAR: i32 = 0;
const FONT_STYLE_BOLD: i32 = 1;
const FORMAT_NO_FIT_BLACK_BOX: i32 = 0x0004;
const FORMAT_NO_WRAP: i32 = 0x1000;
const FORMAT_NO_CLIP: i32 = 0x4000;
const ICON_SIZE: i32 = 32;

/// Keeps GDI+ initialized for as long as the value lives.
pub struct GdiPlus(usize);

impl GdiPlus {
    pub fn start() -> Option<Self> {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 0,
        };
        let mut token = 0usize;
        // SAFETY: the input and token outlive the call; no output struct is requested.
        let status = unsafe { GdiplusStartup(&mut token, &input, null_mut()) };
        (status == STATUS_OK).then_some(Self(token))
    }
}

impl Drop for GdiPlus {
    fn drop(&mut self) {
        // SAFETY: the token came from a successful GdiplusStartup.
        unsafe { GdiplusShutdown(self.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// The x coordinate just past the right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// The y coordinate just past the bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// The rectangle grown by `amount` on every side, or shrunk when negative.
    pub fn inflate(&self, amount: f32) -> Self {
        Self::new(
            self.x - amount,
            self.y - amount,
            (self.w + 2.0 * amount).max(0.0),
            (self.h + 2.0 * amount).max(0.0),
        )
    }

    /// The part shared with `other`, or `None` when they do not overlap.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then(|| Self::new(left, top, right - left, bottom - top))
    }

    fn to_gdip(self) -> RectF {
        RectF {
            X: self.x,
            Y: self.y,
            Width: self.w,
            Height: self.h,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Align {
    Near = 0,
    Center = 1,
    Far = 2,
}

/// How a broken line alternates between ink and gaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dash {
    /// Dashes three line widths long, one line width apart.
    Dashes,
    /// Squares one line width apart.
    Dots,
}

pub struct Font {
    family: *mut GpFontFamily,
    font: *mut GpFont,
}

impl Font {
    /// Creates a pixel-sized font from the first installed family in `families`.
    pub fn new(families: &[&str], size: f32, bold: bool) -> Option<Self> {
        let style = if bold {
            FONT_STYLE_BOLD
        } else {
            FONT_STYLE_REGULAR
        };
        families.iter().find_map(|name| {
            let family = font_family(name)?;
            let mut font = null_mut();
            // SAFETY: the family is valid; on failure it is released below.
            let status = unsafe { GdipCreateFont(family, size, style, UnitPixel, &mut font) };
            if status == STATUS_OK && !font.is_null() {
                Some(Self { family, font })
            } else {
                // SAFETY: the family was created above and is not used again.
                unsafe { GdipDeleteFontFamily(family) };
                None
            }
        })
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: both pointers are owned by this value and released once.
        unsafe {
            GdipDeleteFont(self.font);
            GdipDeleteFontFamily(self.family);
        }
    }
}

fn font_family(name: &str) -> Option<*mut GpFontFamily> {
    let name = wide(name);
    let mut family = null_mut();
    // SAFETY: the name is null-terminated and outlives the call.
    let status = unsafe { GdipCreateFontFamilyFromName(name.as_ptr(), null_mut(), &mut family) };
    (status == STATUS_OK && !family.is_null()).then_some(family)
}

/// A drawing surface. Coordinates are device pixels with pixel edges on integers.
pub struct Graphics(*mut GpGraphics);

impl Graphics {
    pub fn from_hdc(hdc: HDC) -> Option<Self> {
        let mut graphics = null_mut();
        // SAFETY: the caller keeps the device context alive while this value exists.
        let status = unsafe { GdipCreateFromHDC(hdc, &mut graphics) };
        Self::configured(status, graphics)
    }

    fn configured(status: i32, graphics: *mut GpGraphics) -> Option<Self> {
        if status != STATUS_OK || graphics.is_null() {
            return None;
        }
        // SAFETY: the graphics pointer was just created.
        unsafe {
            GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
            GdipSetPixelOffsetMode(graphics, PixelOffsetModeHalf);
            GdipSetTextRenderingHint(graphics, TextRenderingHintClearTypeGridFit);
        }
        Some(Self(graphics))
    }

    pub fn clear(&self, color: u32) {
        // SAFETY: the graphics pointer is valid for the lifetime of self.
        unsafe {
            GdipGraphicsClear(self.0, color);
        }
    }

    pub fn fill_rect(&self, color: u32, rect: Rect) {
        with_brush(color, |brush| {
            // SAFETY: graphics and brush are valid for the duration of the call.
            unsafe {
                GdipFillRectangle(self.0, brush, rect.x, rect.y, rect.w, rect.h);
            }
        });
    }

    pub fn fill_round_rect(&self, color: u32, rect: Rect, radius: f32) {
        let Some(path) = round_rect_path(rect, radius) else {
            return;
        };
        with_brush(color, |brush| {
            // SAFETY: graphics, brush and path are valid for the duration of the call.
            unsafe {
                GdipFillPath(self.0, brush, path);
            }
        });
        // SAFETY: the path was created above and is not used again.
        unsafe {
            GdipDeletePath(path);
        }
    }

    /// Strokes a one-device-pixel outline just inside `rect`.
    pub fn stroke_round_rect(&self, color: u32, rect: Rect, radius: f32) {
        let inset = Rect::new(rect.x + 0.5, rect.y + 0.5, rect.w - 1.0, rect.h - 1.0);
        let Some(path) = round_rect_path(inset, radius) else {
            return;
        };
        // SAFETY: the pen and path are created, used and released within this block.
        unsafe {
            let mut pen = null_mut();
            if GdipCreatePen1(color, 1.0, UnitWorld, &mut pen) == STATUS_OK {
                GdipDrawPath(self.0, pen, path);
                GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }

    /// Strokes a straight line with rounded ends.
    pub fn line(&self, color: u32, from: (f32, f32), to: (f32, f32), width: f32) {
        // SAFETY: the pen is created, used and released within this block.
        unsafe {
            let mut pen = null_mut();
            if GdipCreatePen1(color, width, UnitWorld, &mut pen) == STATUS_OK {
                GdipSetPenStartCap(pen, LineCapRound);
                GdipSetPenEndCap(pen, LineCapRound);
                GdipDrawLine(self.0, pen, from.0, from.1, to.0, to.1);
                GdipDeletePen(pen);
            }
        }
    }

    /// Strokes part of the ellipse inscribed in `rect` with rounded ends.
    /// Angles are in degrees, clockwise, with zero pointing right.
    pub fn arc(&self, color: u32, rect: Rect, start: f32, sweep: f32, width: f32) {
        // SAFETY: the pen is created, used and released within this block.
        unsafe {
            let mut pen = null_mut();
            if GdipCreatePen1(color, width, UnitWorld, &mut pen) == STATUS_OK {
                GdipSetPenStartCap(pen, LineCapRound);
                GdipSetPenEndCap(pen, LineCapRound);
                GdipDrawArc(self.0, pen, rect.x, rect.y, rect.w, rect.h, start, sweep);
                GdipDeletePen(pen);
            }
        }
    }

    /// Strokes an outline `width` device pixels thick just inside `rect`.
    pub fn stroke_round_rect_wide(&self, color: u32, rect: Rect, radius: f32, width: f32) {
        let half = width / 2.0;
        let inset = Rect::new(rect.x + half, rect.y + half, rect.w - width, rect.h - width);
        let Some(path) = round_rect_path(inset, (radius - half).max(0.0)) else {
            return;
        };
        // SAFETY: the pen and path are created, used and released within this block.
        unsafe {
            let mut pen = null_mut();
            if GdipCreatePen1(color, width, UnitWorld, &mut pen) == STATUS_OK {
                GdipDrawPath(self.0, pen, path);
                GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }

    /// Strokes connected line segments through `points` with rounded ends
    /// and corners.
    pub fn polyline(&self, color: u32, points: &[(f32, f32)], width: f32) {
        if points.len() < 2 {
            return;
        }
        let points: Vec<PointF> = points.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
        // SAFETY: the pen is created, used and released within this block, and
        // the point count passed matches the vector.
        unsafe {
            let mut pen = null_mut();
            if GdipCreatePen1(color, width, UnitWorld, &mut pen) == STATUS_OK {
                GdipSetPenStartCap(pen, LineCapRound);
                GdipSetPenEndCap(pen, LineCapRound);
                GdipSetPenLineJoin(pen, LineJoinRound);
                GdipDrawLines(self.0, pen, points.as_ptr(), points.len() as i32);
                GdipDeletePen(pen);
            }
        }
    }

    /// Strokes connected line segments through `points` as a broken line
    /// with square ends. The pattern runs on across the corners.
    pub fn dashed_polyline(&self, color: u32, points: &[(f32, f32)], width: f32, dash: Dash) {
        if points.len() < 2 {
            return;
        }
        let points: Vec<PointF> = points.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
        let style = match dash {
            Dash::Dashes => DashStyleDash,
            Dash::Dots => DashStyleDot,
        };
        // SAFETY: the pen is created, used and released within this block, and
        // the point count passed matches the vector.
        unsafe {
            let mut pen = null_mut();
            if GdipCreatePen1(color, width, UnitWorld, &mut pen) == STATUS_OK {
                GdipSetPenDashStyle(pen, style);
                GdipDrawLines(self.0, pen, points.as_ptr(), points.len() as i32);
                GdipDeletePen(pen);
            }
        }
    }

    /// Fills the closed shape whose corners are `points`.
    pub fn fill_polygon(&self, color: u32, points: &[(f32, f32)]) {
        if points.len() < 3 {
            return;
        }
        let points: Vec<PointF> = points.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
        with_brush(color, |brush| {
            // SAFETY: graphics and brush are valid for the duration of the
            // call, and the point count passed matches the vector.
            unsafe {
                GdipFillPolygon(
                    self.0,
                    brush,
                    points.as_ptr(),
                    points.len() as i32,
                    FillModeAlternate,
                );
            }
        });
    }

    /// Fills the ellipse inscribed in `rect`.
    pub fn fill_ellipse(&self, color: u32, rect: Rect) {
        with_brush(color, |brush| {
            // SAFETY: graphics and brush are valid for the duration of the call.
            unsafe {
                GdipFillEllipse(self.0, brush, rect.x, rect.y, rect.w, rect.h);
            }
        });
    }

    /// Strokes the ellipse inscribed in `rect`, keeping the line inside it.
    pub fn stroke_ellipse(&self, color: u32, rect: Rect, width: f32) {
        let half = width / 2.0;
        // SAFETY: the pen is created, used and released within this block.
        unsafe {
            let mut pen = null_mut();
            if GdipCreatePen1(color, width, UnitWorld, &mut pen) == STATUS_OK {
                GdipDrawEllipse(
                    self.0,
                    pen,
                    rect.x + half,
                    rect.y + half,
                    rect.w - width,
                    rect.h - width,
                );
                GdipDeletePen(pen);
            }
        }
    }

    /// Limits all later drawing to `rect` until `reset_clip` is called.
    pub fn set_clip(&self, rect: Rect) {
        // SAFETY: the graphics pointer is valid for the lifetime of self.
        unsafe {
            GdipSetClipRect(self.0, rect.x, rect.y, rect.w, rect.h, CombineModeReplace);
        }
    }

    /// Removes the limit set by `set_clip`.
    pub fn reset_clip(&self) {
        // SAFETY: the graphics pointer is valid for the lifetime of self.
        unsafe {
            GdipResetClip(self.0);
        }
    }

    /// Draws a tray icon's pixels scaled to fill `rect`.
    pub fn icon(&self, icon: &IconBitmap, rect: Rect) {
        // SAFETY: the graphics and bitmap pointers are valid for the call.
        unsafe {
            GdipSetInterpolationMode(self.0, InterpolationModeHighQualityBicubic);
            GdipDrawImageRect(self.0, icon.0.cast(), rect.x, rect.y, rect.w, rect.h);
        }
    }

    /// Draws one line of text aligned within `rect`. A zero width or height
    /// leaves that dimension unbounded.
    pub fn text(
        &self,
        text: &str,
        font: &Font,
        color: u32,
        rect: Rect,
        horizontal: Align,
        vertical: Align,
    ) {
        self.draw_string(text, font, color, rect, horizontal, vertical, false);
    }

    /// Draws text from the top-left of `rect`, wrapping at its width.
    pub fn text_wrapped(&self, text: &str, font: &Font, color: u32, rect: Rect) {
        self.draw_string(text, font, color, rect, Align::Near, Align::Near, true);
    }

    /// Size of one unwrapped line of text.
    pub fn measure(&self, text: &str, font: &Font) -> (f32, f32) {
        self.measure_string(text, font, 0.0, false)
    }

    /// Height of text wrapped at `width`.
    pub fn measure_wrapped(&self, text: &str, font: &Font, width: f32) -> f32 {
        self.measure_string(text, font, width, true).1
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_string(
        &self,
        text: &str,
        font: &Font,
        color: u32,
        rect: Rect,
        horizontal: Align,
        vertical: Align,
        wrap: bool,
    ) {
        let text = wide(text);
        let layout = rect.to_gdip();
        with_string_format(horizontal, vertical, wrap, |format| {
            with_brush(color, |brush| {
                // SAFETY: all pointers are valid for the call; -1 reads to the terminator.
                unsafe {
                    GdipDrawString(self.0, text.as_ptr(), -1, font.font, &layout, format, brush);
                }
            });
        });
    }

    fn measure_string(&self, text: &str, font: &Font, width: f32, wrap: bool) -> (f32, f32) {
        let text = wide(text);
        let layout = RectF {
            X: 0.0,
            Y: 0.0,
            Width: width,
            Height: 0.0,
        };
        let mut bounds = RectF {
            X: 0.0,
            Y: 0.0,
            Width: 0.0,
            Height: 0.0,
        };
        with_string_format(Align::Near, Align::Near, wrap, |format| {
            // SAFETY: all pointers are valid for the call; -1 reads to the terminator.
            unsafe {
                GdipMeasureString(
                    self.0,
                    text.as_ptr(),
                    -1,
                    font.font,
                    &layout,
                    format,
                    &mut bounds,
                    null_mut(),
                    null_mut(),
                );
            }
        });
        (bounds.Width.ceil(), bounds.Height.ceil())
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        // SAFETY: the pointer is owned by this value and released once.
        unsafe {
            GdipDeleteGraphics(self.0);
        }
    }
}

fn with_brush(color: u32, draw: impl FnOnce(*mut GpBrush)) {
    let mut brush: *mut GpSolidFill = null_mut();
    // SAFETY: the brush is created, lent to the closure and released here.
    unsafe {
        if GdipCreateSolidFill(color, &mut brush) == STATUS_OK && !brush.is_null() {
            draw(brush.cast());
            GdipDeleteBrush(brush.cast());
        }
    }
}

fn with_string_format(
    horizontal: Align,
    vertical: Align,
    wrap: bool,
    draw: impl FnOnce(*const GpStringFormat),
) {
    let Some(format) = typographic_format(if wrap { 0 } else { FORMAT_NO_WRAP }) else {
        return;
    };
    // SAFETY: the format was just created and is released after the closure runs.
    unsafe {
        GdipSetStringFormatAlign(format, horizontal as i32);
        GdipSetStringFormatLineAlign(format, vertical as i32);
        draw(format);
        GdipDeleteStringFormat(format);
    }
}

/// A private copy of the typographic format, which measures text without padding.
fn typographic_format(extra_flags: i32) -> Option<*mut GpStringFormat> {
    let mut generic = null_mut();
    let mut format = null_mut();
    // SAFETY: the generic format is a GDI+ singleton; only the clone is modified.
    unsafe {
        if GdipStringFormatGetGenericTypographic(&mut generic) != STATUS_OK
            || GdipCloneStringFormat(generic, &mut format) != STATUS_OK
            || format.is_null()
        {
            return None;
        }
        GdipSetStringFormatFlags(
            format,
            FORMAT_NO_FIT_BLACK_BOX | FORMAT_NO_CLIP | extra_flags,
        );
    }
    Some(format)
}

fn round_rect_path(rect: Rect, radius: f32) -> Option<*mut GpPath> {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    let diameter = (radius * 2.0).min(rect.w).min(rect.h).max(0.01);
    let right = rect.x + rect.w - diameter;
    let bottom = rect.y + rect.h - diameter;
    let mut path = null_mut();
    // SAFETY: the path is created here and owned by the caller on success.
    unsafe {
        if GdipCreatePath(FillModeAlternate, &mut path) != STATUS_OK || path.is_null() {
            return None;
        }
        GdipAddPathArc(path, rect.x, rect.y, diameter, diameter, 180.0, 90.0);
        GdipAddPathArc(path, right, rect.y, diameter, diameter, 270.0, 90.0);
        GdipAddPathArc(path, right, bottom, diameter, diameter, 0.0, 90.0);
        GdipAddPathArc(path, rect.x, bottom, diameter, diameter, 90.0, 90.0);
        GdipClosePathFigure(path);
    }
    Some(path)
}

/// Renders a 32x32 tray icon with `text` fitted into the square, using the
/// same text bounds and font choices as the existing tray icon. Without a
/// `background` the text is drawn on a transparent icon.
pub fn render_text_icon(text: &str, background: Option<u32>, foreground: u32) -> Option<HICON> {
    let bitmap = render_text_bitmap(text, background, foreground)?;
    let mut icon: HICON = null_mut();
    // SAFETY: the bitmap is valid and released after the icon is created.
    unsafe {
        let status = GdipCreateHICONFromBitmap(bitmap, &mut icon);
        GdipDisposeImage(bitmap.cast());
        (status == STATUS_OK && !icon.is_null()).then_some(icon)
    }
}

/// The pixels of a tray icon, kept so a window can show what the tray shows.
pub struct IconBitmap(*mut GpBitmap);

impl IconBitmap {
    /// Renders exactly the image `render_text_icon` turns into a tray icon.
    pub fn render(text: &str, background: Option<u32>, foreground: u32) -> Option<Self> {
        render_text_bitmap(text, background, foreground).map(Self)
    }
}

impl Drop for IconBitmap {
    fn drop(&mut self) {
        // SAFETY: the bitmap is owned by this value and released once.
        unsafe {
            GdipDisposeImage(self.0.cast());
        }
    }
}

fn render_text_bitmap(
    text: &str,
    background: Option<u32>,
    foreground: u32,
) -> Option<*mut GpBitmap> {
    let length = text.chars().count();
    let bounds = match text {
        "!" => Rect::new(4.0, 4.0, 24.0, 24.0),
        "100" => Rect::new(3.0, 6.0, 26.0, 18.0),
        _ if length >= 3 => Rect::new(1.0, 3.0, 30.0, 25.0),
        _ => Rect::new(1.0, 6.0, 30.0, 20.0),
    };
    let families: &[&str] = match text {
        "!" => &["Segoe UI", "Microsoft Sans Serif"],
        _ if length >= 3 => &[
            "Segoe UI Semibold",
            "Arial Narrow",
            "Segoe UI",
            "Microsoft Sans Serif",
        ],
        _ => &["Segoe UI Semibold", "Segoe UI", "Microsoft Sans Serif"],
    };
    // Two-digit values are drawn slightly smaller so they do not touch the edges.
    let squeeze = if length == 2 { 0.92 } else { 1.0 };

    let mut bitmap: *mut GpBitmap = null_mut();
    // SAFETY: every GDI+ object created below is released before returning,
    // except the bitmap, which is returned to the caller on success.
    unsafe {
        let created = GdipCreateBitmapFromScan0(
            ICON_SIZE,
            ICON_SIZE,
            0,
            PIXEL_FORMAT_32BPP_ARGB,
            null(),
            &mut bitmap,
        );
        if created != STATUS_OK || bitmap.is_null() {
            return None;
        }
        let mut graphics = null_mut();
        if GdipGetImageGraphicsContext(bitmap.cast::<GpImage>(), &mut graphics) != STATUS_OK
            || graphics.is_null()
        {
            GdipDisposeImage(bitmap.cast());
            return None;
        }
        GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        GdipGraphicsClear(graphics, 0);
        if let Some(background) = background {
            with_brush(background, |brush| {
                let edge = ICON_SIZE as f32 + 4.0;
                GdipFillRectangle(graphics, brush, -2.0, -2.0, edge, edge);
            });
        }

        if let Some(path) = fitted_text_path(text, families, bounds, squeeze) {
            with_brush(foreground, |brush| {
                GdipFillPath(graphics, brush, path);
            });
            GdipDeletePath(path);
        }
        GdipDeleteGraphics(graphics);
    }
    Some(bitmap)
}

/// Outlines `text` and scales it uniformly to fill `bounds`, centered.
fn fitted_text_path(
    text: &str,
    families: &[&str],
    bounds: Rect,
    squeeze: f32,
) -> Option<*mut GpPath> {
    let family = families.iter().find_map(|name| font_family(name))?;
    let format = typographic_format(FORMAT_NO_WRAP);
    let text = wide(text);
    let origin = RectF {
        X: 0.0,
        Y: 0.0,
        Width: 0.0,
        Height: 0.0,
    };
    let mut path = null_mut();
    // SAFETY: the family and format are released here; the path is released
    // here on failure and owned by the caller on success.
    unsafe {
        let mut outlined = false;
        if let Some(format) = format {
            if GdipCreatePath(FillModeAlternate, &mut path) == STATUS_OK && !path.is_null() {
                let mut natural = origin;
                outlined = GdipAddPathString(
                    path,
                    text.as_ptr(),
                    -1,
                    family,
                    FONT_STYLE_REGULAR,
                    100.0,
                    &origin,
                    format,
                ) == STATUS_OK
                    && GdipGetPathWorldBounds(path, &mut natural, null(), null()) == STATUS_OK
                    && natural.Width > 0.0
                    && natural.Height > 0.0;
                if outlined {
                    let scale =
                        squeeze * f32::min(bounds.w / natural.Width, bounds.h / natural.Height);
                    let dx = bounds.x + bounds.w / 2.0 - scale * (natural.X + natural.Width / 2.0);
                    let dy = bounds.y + bounds.h / 2.0 - scale * (natural.Y + natural.Height / 2.0);
                    let mut matrix = null_mut();
                    outlined =
                        GdipCreateMatrix2(scale, 0.0, 0.0, scale, dx, dy, &mut matrix) == STATUS_OK;
                    if outlined {
                        GdipTransformPath(path, matrix);
                        GdipDeleteMatrix(matrix);
                    }
                }
            }
            GdipDeleteStringFormat(format);
        }
        GdipDeleteFontFamily(family);
        if !outlined && !path.is_null() {
            GdipDeletePath(path);
            return None;
        }
        outlined.then_some(path)
    }
}

#[cfg(test)]
pub mod tests {
    use std::path::{Path, PathBuf};

    use windows_sys::core::GUID;
    use windows_sys::Win32::Graphics::GdiPlus::{GdipBitmapGetPixel, GdipSaveImageToFile};

    use super::*;

    const PNG_ENCODER: GUID = GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);

    /// An off-screen ARGB image that tests can draw on and save.
    pub struct Canvas(*mut GpBitmap);

    impl Canvas {
        pub fn new(width: i32, height: i32) -> Self {
            let mut bitmap = null_mut();
            // SAFETY: GDI+ allocates the pixels; the bitmap is released on drop.
            let status = unsafe {
                GdipCreateBitmapFromScan0(
                    width,
                    height,
                    0,
                    PIXEL_FORMAT_32BPP_ARGB,
                    null(),
                    &mut bitmap,
                )
            };
            assert_eq!(status, STATUS_OK);
            Self(bitmap)
        }

        pub fn graphics(&self) -> Graphics {
            let mut graphics = null_mut();
            // SAFETY: the bitmap outlives the returned graphics in every test.
            let status = unsafe { GdipGetImageGraphicsContext(self.0.cast(), &mut graphics) };
            Graphics::configured(status, graphics).expect("graphics for canvas")
        }

        /// The ARGB color of one pixel.
        pub fn pixel(&self, x: i32, y: i32) -> u32 {
            let mut color = 0u32;
            // SAFETY: the bitmap is valid and the color is written to a local.
            let status = unsafe { GdipBitmapGetPixel(self.0, x, y, &mut color) };
            assert_eq!(status, STATUS_OK, "pixel ({x}, {y})");
            color
        }

        pub fn save_png(&self, path: &Path) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = wide(&path.to_string_lossy());
            // SAFETY: the bitmap, file name and encoder id are valid for the call.
            let status =
                unsafe { GdipSaveImageToFile(self.0.cast(), file.as_ptr(), &PNG_ENCODER, null()) };
            assert_eq!(status, STATUS_OK, "saving {}", path.display());
        }
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            // SAFETY: the bitmap is owned by this value and released once.
            unsafe {
                GdipDisposeImage(self.0.cast());
            }
        }
    }

    /// Where the ignored sample-writing tests put their images.
    pub fn sample_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("samples")
            .join(name)
    }

    #[test]
    fn rectangles_report_their_edges_and_overlap() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!((rect.right(), rect.bottom()), (40.0, 60.0));
        assert_eq!(rect.inflate(2.0), Rect::new(8.0, 18.0, 34.0, 44.0));
        assert_eq!(rect.inflate(-50.0), Rect::new(60.0, 70.0, 0.0, 0.0));
        assert_eq!(
            rect.intersection(&Rect::new(30.0, 50.0, 100.0, 100.0)),
            Some(Rect::new(30.0, 50.0, 10.0, 10.0))
        );
        assert_eq!(rect.intersection(&Rect::new(40.0, 20.0, 5.0, 5.0)), None);
    }

    #[test]
    fn clipping_limits_drawing_until_it_is_reset() {
        const INK: u32 = 0xFF11_2233;
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(40, 40);
        let g = canvas.graphics();
        g.clear(0xFFFF_FFFF);
        g.set_clip(Rect::new(0.0, 0.0, 20.0, 40.0));
        g.fill_rect(INK, Rect::new(0.0, 0.0, 40.0, 10.0));
        assert_eq!(canvas.pixel(5, 5), INK);
        assert_eq!(canvas.pixel(30, 5), 0xFFFF_FFFF);
        g.reset_clip();
        g.fill_rect(INK, Rect::new(0.0, 20.0, 40.0, 10.0));
        assert_eq!(canvas.pixel(30, 25), INK);
    }

    #[test]
    fn shapes_cover_the_pixels_they_describe() {
        const INK: u32 = 0xFF11_2233;
        const PAPER: u32 = 0xFFFF_FFFF;
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(60, 60);
        let g = canvas.graphics();
        g.clear(PAPER);

        g.fill_ellipse(INK, Rect::new(0.0, 0.0, 20.0, 20.0));
        assert_eq!(canvas.pixel(10, 10), INK);
        assert_eq!(canvas.pixel(0, 0), PAPER);

        g.stroke_ellipse(INK, Rect::new(20.0, 0.0, 20.0, 20.0), 2.0);
        assert_eq!(canvas.pixel(21, 10), INK);
        assert_eq!(canvas.pixel(30, 10), PAPER);

        g.stroke_round_rect_wide(INK, Rect::new(0.0, 20.0, 20.0, 20.0), 4.0, 2.0);
        assert_eq!(canvas.pixel(10, 21), INK);
        assert_eq!(canvas.pixel(10, 30), PAPER);

        g.polyline(INK, &[(22.0, 30.0), (30.0, 30.0), (38.0, 30.0)], 2.0);
        assert_eq!(canvas.pixel(30, 30), INK);
        assert_eq!(canvas.pixel(30, 36), PAPER);
    }

    #[test]
    fn broken_lines_leave_gaps_and_polygons_are_filled() {
        const INK: u32 = 0xFF11_2233;
        const PAPER: u32 = 0xFFFF_FFFF;
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(60, 40);
        let g = canvas.graphics();
        g.clear(PAPER);

        // Two pixels wide: dashes of six pixels, gaps of two.
        g.dashed_polyline(INK, &[(0.0, 5.0), (40.0, 5.0)], 2.0, Dash::Dashes);
        let row = |y: i32| -> Vec<bool> { (0..40).map(|x| canvas.pixel(x, y) == INK).collect() };
        let dashes = row(4);
        assert!(dashes[..6].iter().all(|&ink| ink) && !dashes[6] && !dashes[7] && dashes[8]);
        assert_eq!(dashes.iter().filter(|&&ink| ink).count(), 30);

        // Dots and gaps of two pixels each.
        g.dashed_polyline(INK, &[(0.0, 15.0), (40.0, 15.0)], 2.0, Dash::Dots);
        let dots = row(14);
        assert!(dots[0] && dots[1] && !dots[2] && !dots[3] && dots[4]);
        assert_eq!(dots.iter().filter(|&&ink| ink).count(), 20);

        // Fewer than two points draw nothing.
        g.dashed_polyline(INK, &[(0.0, 25.0)], 2.0, Dash::Dashes);
        assert_eq!(canvas.pixel(0, 25), PAPER);

        g.fill_polygon(INK, &[(40.0, 20.0), (60.0, 20.0), (50.0, 40.0)]);
        assert_eq!(canvas.pixel(50, 25), INK);
        assert_eq!(canvas.pixel(42, 38), PAPER);
        g.fill_polygon(INK, &[(0.0, 30.0), (10.0, 30.0)]);
        assert_eq!(canvas.pixel(5, 30), PAPER);
    }

    #[test]
    fn icon_previews_draw_the_tray_icon_pixels() {
        const BOX: u32 = 0xFF00_78D7;
        let _gdiplus = GdiPlus::start().unwrap();
        let icon = IconBitmap::render("50", Some(BOX), 0xFFFF_FFFF).unwrap();
        let canvas = Canvas::new(40, 40);
        let g = canvas.graphics();
        g.clear(0xFF00_0000);
        g.icon(&icon, Rect::new(4.0, 4.0, 32.0, 32.0));
        // The box fills the icon; the digits are drawn in the text color.
        assert_eq!(canvas.pixel(6, 6), BOX);
        assert_eq!(canvas.pixel(1, 1), 0xFF00_0000);
        let has_text = (4..36).any(|x| canvas.pixel(x, 20) == 0xFFFF_FFFF);
        assert!(has_text);

        // Without a box only the digits are drawn.
        let bare = IconBitmap::render("50", None, 0xFFFF_FFFF).unwrap();
        g.clear(0xFF00_0000);
        g.icon(&bare, Rect::new(4.0, 4.0, 32.0, 32.0));
        assert_eq!(canvas.pixel(6, 6), 0xFF00_0000);
    }

    /// Writes tray icons for comparison with the existing app's renderer.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "writes sample images to target/samples"]
    fn write_icon_samples() {
        let _gdiplus = GdiPlus::start().unwrap();
        let cases: [(&str, &str, Option<u32>, u32); 9] = [
            ("black-5", "5", Some(0xFF00_0000), 0xFFFF_FFFF),
            ("black-42", "42", Some(0xFF00_0000), 0xFFFF_FFFF),
            ("black-72", "72", Some(0xFF00_0000), 0xFFFF_FFFF),
            ("black-100", "100", Some(0xFF00_0000), 0xFFFF_FFFF),
            ("default-72", "72", Some(0xFF00_78D7), 0xFFFF_FFFF),
            ("default-20", "20", Some(0xFFF0_D200), 0xFFFF_FFFF),
            ("nobox-72", "72", None, 0xFFFF_FFFF),
            ("unknown", "...", Some(0xFF40_4040), 0xFFFF_FFFF),
            ("error", "!", Some(0xFFD2_2E2E), 0xFFFF_FFFF),
        ];
        for (name, text, background, foreground) in cases {
            let bitmap = render_text_bitmap(text, background, foreground).expect(name);
            Canvas(bitmap).save_png(&sample_path(&format!("icon-{name}.png")));
        }
    }
}
