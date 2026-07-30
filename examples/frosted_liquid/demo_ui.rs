use std::time::Instant;

use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Gdi::{
                ANTIALIASED_QUALITY, BeginPaint, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen,
                CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_LEFT, DT_RIGHT,
                DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE,
                FW_SEMIBOLD, FillRect, GetStockObject, HBRUSH, HFONT, InvalidateRect, LineTo,
                MoveToEx, NULL_PEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, RoundRect,
                SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
            },
            GdiPlus::{
                FillModeAlternate, GdipAddPathArcI, GdipClosePathFigure, GdipCreateFromHDC,
                GdipCreatePath, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics,
                GdipDeletePath, GdipFillEllipseI, GdipFillPath, GdipFillRectangleI,
                GdipSetSmoothingMode, GdiplusStartup, GdiplusStartupInput, GpBrush, GpGraphics,
                GpPath, GpSolidFill, SmoothingModeAntiAlias8x8,
            },
        },
        UI::{
            Controls::{
                BPBF_COMPATIBLEBITMAP, BeginBufferedPaint, BufferedPaintInit, DTT_COMPOSITED,
                DTT_GLOWSIZE, DTT_SHADOWCOLOR, DTT_SHADOWOFFSET, DTT_SHADOWTYPE, DTT_TEXTCOLOR,
                DTTOPTS, DrawThemeTextEx, EndBufferedPaint, HTHEME, OpenThemeData, TST_CONTINUOUS,
            },
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
            WindowsAndMessaging::*,
        },
    },
    core::{Result, w},
};

use super::{EffectStyle, update_live_style};

const PANEL_COLLAPSED_H: i32 = 52;
const PARAMETER_COUNT: usize = 6;
const PARAMETER_COLUMNS: usize = 2;
const FIRST_SLIDER_Y: i32 = 158;
const PARAMETER_ROW_HEIGHT: i32 = 60;
const FOOTER_GAP: i32 = 22;
const FOOTER_HEIGHT: i32 = 26;
const PANEL_BOTTOM_PADDING: i32 = 16;

fn slider_y(index: usize) -> i32 {
    FIRST_SLIDER_Y + (index / PARAMETER_COLUMNS) as i32 * PARAMETER_ROW_HEIGHT
}

fn footer_top() -> i32 {
    let rows = PARAMETER_COUNT.div_ceil(PARAMETER_COLUMNS) as i32;
    FIRST_SLIDER_Y + (rows - 1) * PARAMETER_ROW_HEIGHT + FOOTER_GAP
}

fn expanded_height() -> i32 {
    footer_top() + FOOTER_HEIGHT + PANEL_BOTTOM_PADDING
}

static mut STYLE: Option<EffectStyle> = None;
static mut EXPANDED: bool = false;
static mut ACTIVE_SLIDER: Option<usize> = None;
static mut PANEL_WINDOW: Option<HWND> = None;
static mut INPUT_WINDOW: Option<HWND> = None;
static mut PANEL_HEIGHT: f32 = PANEL_COLLAPSED_H as f32;
static mut LAST_TICK: Option<Instant> = None;
static mut PANEL_WIDTH: i32 = 420;
static mut UI_SCALE: f32 = 1.0;
static mut PAINT_SCALE: f32 = 1.0;
static mut GDIPLUS_TOKEN: usize = 0;
static mut PAINT_GRAPHICS: *mut GpGraphics = std::ptr::null_mut();
static mut FONT_BODY: Option<HFONT> = None;
static mut FONT_LABEL: Option<HFONT> = None;
static mut FONT_TITLE: Option<HFONT> = None;
static mut FONT_DISPLAY: Option<HFONT> = None;
static mut TEXT_THEME: Option<HTHEME> = None;

pub unsafe fn tick() {
    let Some(panel) = PANEL_WINDOW else { return };
    let now = Instant::now();
    let dt = LAST_TICK.map_or(1.0 / 60.0, |last| {
        now.duration_since(last).as_secs_f32().min(0.05)
    });
    LAST_TICK = Some(now);
    let target = if EXPANDED {
        expanded_height() as f32 * UI_SCALE
    } else {
        PANEL_COLLAPSED_H as f32 * UI_SCALE
    };
    let previous_height = PANEL_HEIGHT.round() as i32;
    let blend = 1.0 - (-dt * 18.0).exp();
    PANEL_HEIGHT += (target - PANEL_HEIGHT) * blend;
    if (PANEL_HEIGHT - target).abs() < 0.5 {
        PANEL_HEIGHT = target;
    }
    let height = PANEL_HEIGHT.round() as i32;
    if height != previous_height {
        let _ = SetWindowPos(
            panel,
            Some(HWND_TOPMOST),
            0,
            0,
            PANEL_WIDTH,
            height,
            SWP_NOMOVE | SWP_NOACTIVATE,
        );
        let region = windows::Win32::Graphics::Gdi::CreateRoundRectRgn(
            0,
            0,
            PANEL_WIDTH + 1,
            height + 1,
            (16.0 * UI_SCALE).round() as i32,
            (16.0 * UI_SCALE).round() as i32,
        );
        let _ = SetWindowRgn(panel, Some(region), true);
        let _ = InvalidateRect(Some(panel), None, false);
    }
    if let Some(input) = INPUT_WINDOW {
        let mut rect = RECT::default();
        let _ = GetWindowRect(panel, &mut rect);
        let input_height = height.max((64.0 * UI_SCALE).round() as i32);
        let _ = SetWindowPos(
            input,
            Some(HWND_TOPMOST),
            rect.left,
            rect.top,
            PANEL_WIDTH,
            input_height,
            SWP_NOACTIVATE,
        );
    }
}

pub unsafe fn glass_extension() -> i32 {
    if PANEL_WINDOW.is_some() {
        (PANEL_HEIGHT.round() as i32 - (PANEL_COLLAPSED_H as f32 * UI_SCALE).round() as i32).max(0)
    } else {
        0
    }
}

pub unsafe fn collapsed_height() -> i32 {
    (PANEL_COLLAPSED_H as f32 * UI_SCALE).round() as i32
}

fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | ((green as u32) << 8) | ((blue as u32) << 16))
}

fn scaled(value: i32) -> i32 {
    unsafe { (value as f32 * PAINT_SCALE).round() as i32 }
}

fn scaled_rect(rect: RECT) -> RECT {
    RECT {
        left: scaled(rect.left),
        top: scaled(rect.top),
        right: scaled(rect.right),
        bottom: scaled(rect.bottom),
    }
}

fn argb(color: COLORREF) -> u32 {
    let raw = color.0;
    0xff00_0000 | ((raw & 0xff) << 16) | (raw & 0xff00) | ((raw >> 16) & 0xff)
}

unsafe fn begin_antialiased_geometry(hdc: windows::Win32::Graphics::Gdi::HDC) {
    PAINT_GRAPHICS = std::ptr::null_mut();
    let _ = GdipCreateFromHDC(hdc, &mut PAINT_GRAPHICS);
    if !PAINT_GRAPHICS.is_null() {
        let _ = GdipSetSmoothingMode(PAINT_GRAPHICS, SmoothingModeAntiAlias8x8);
    }
}

unsafe fn end_antialiased_geometry() {
    if !PAINT_GRAPHICS.is_null() {
        let _ = GdipDeleteGraphics(PAINT_GRAPHICS);
        PAINT_GRAPHICS = std::ptr::null_mut();
    }
}

unsafe fn gdiplus_brush(color: COLORREF) -> *mut GpSolidFill {
    let mut brush = std::ptr::null_mut();
    let _ = GdipCreateSolidFill(argb(color), &mut brush);
    brush
}

unsafe fn text(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    value: &str,
    rect: RECT,
    color: COLORREF,
    format: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    fixed_text(hdc, value, rect, color, format);
}

unsafe fn fixed_text(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    value: &str,
    rect: RECT,
    color: COLORREF,
    format: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    let mut utf16: Vec<u16> = value.encode_utf16().collect();
    let mut bounds = scaled_rect(rect);
    if let Some(theme) = TEXT_THEME {
        let over_light = STYLE.is_some_and(|style| style.over_light);
        let options = DTTOPTS {
            dwSize: std::mem::size_of::<DTTOPTS>() as u32,
            dwFlags: if over_light {
                DTT_TEXTCOLOR | DTT_COMPOSITED
            } else {
                DTT_TEXTCOLOR
                    | DTT_SHADOWCOLOR
                    | DTT_SHADOWTYPE
                    | DTT_SHADOWOFFSET
                    | DTT_GLOWSIZE
                    | DTT_COMPOSITED
            },
            crText: color,
            crShadow: rgb(0, 0, 0),
            iTextShadowType: TST_CONTINUOUS.0,
            ptShadowOffset: POINT { x: 0, y: scaled(2) },
            iGlowSize: scaled(6),
            ..Default::default()
        };
        if DrawThemeTextEx(
            theme,
            hdc,
            0,
            0,
            &utf16,
            format,
            &mut bounds,
            Some(&options),
        )
        .is_ok()
        {
            return;
        }
    }
    SetTextColor(hdc, color);
    DrawTextW(hdc, &mut utf16, &mut bounds, format);
}

unsafe fn solid_brush(color: COLORREF) -> HBRUSH {
    CreateSolidBrush(color)
}

unsafe fn fill_rect(hdc: windows::Win32::Graphics::Gdi::HDC, rect: RECT, color: COLORREF) {
    let rect = scaled_rect(rect);
    if !PAINT_GRAPHICS.is_null() {
        let brush = gdiplus_brush(color);
        if !brush.is_null() {
            let _ = GdipFillRectangleI(
                PAINT_GRAPHICS,
                brush.cast::<GpBrush>(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            );
            let _ = GdipDeleteBrush(brush.cast::<GpBrush>());
            return;
        }
    }
    let brush = solid_brush(color);
    FillRect(hdc, &rect, brush);
    let _ = DeleteObject(brush.into());
}

unsafe fn select_font(hdc: windows::Win32::Graphics::Gdi::HDC, font: Option<HFONT>) {
    if let Some(font) = font {
        let _ = SelectObject(hdc, font.into());
    }
}

unsafe fn make_font(height: i32, weight: i32) -> HFONT {
    CreateFontW(
        -height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
        w!("Segoe UI"),
    )
}

unsafe fn fill_round_rect(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    rect: RECT,
    radius: i32,
    color: COLORREF,
) {
    let rect = scaled_rect(rect);
    if !PAINT_GRAPHICS.is_null() {
        let diameter = scaled(radius).max(1);
        let brush = gdiplus_brush(color);
        let mut path: *mut GpPath = std::ptr::null_mut();
        let _ = GdipCreatePath(FillModeAlternate, &mut path);
        if !brush.is_null() && !path.is_null() {
            let _ = GdipAddPathArcI(path, rect.left, rect.top, diameter, diameter, 180.0, 90.0);
            let _ = GdipAddPathArcI(
                path,
                rect.right - diameter,
                rect.top,
                diameter,
                diameter,
                270.0,
                90.0,
            );
            let _ = GdipAddPathArcI(
                path,
                rect.right - diameter,
                rect.bottom - diameter,
                diameter,
                diameter,
                0.0,
                90.0,
            );
            let _ = GdipAddPathArcI(
                path,
                rect.left,
                rect.bottom - diameter,
                diameter,
                diameter,
                90.0,
                90.0,
            );
            let _ = GdipClosePathFigure(path);
            let _ = GdipFillPath(PAINT_GRAPHICS, brush.cast::<GpBrush>(), path);
        }
        if !path.is_null() {
            let _ = GdipDeletePath(path);
        }
        if !brush.is_null() {
            let _ = GdipDeleteBrush(brush.cast::<GpBrush>());
        }
        return;
    }
    let brush = solid_brush(color);
    let old = SelectObject(hdc, brush.into());
    let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
    let _ = RoundRect(
        hdc,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        scaled(radius),
        scaled(radius),
    );
    SelectObject(hdc, old_pen);
    SelectObject(hdc, old);
    let _ = DeleteObject(brush.into());
}

unsafe fn fill_ellipse(hdc: windows::Win32::Graphics::Gdi::HDC, rect: RECT, color: COLORREF) {
    let rect = scaled_rect(rect);
    if !PAINT_GRAPHICS.is_null() {
        let brush = gdiplus_brush(color);
        if !brush.is_null() {
            let _ = GdipFillEllipseI(
                PAINT_GRAPHICS,
                brush.cast::<GpBrush>(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            );
            let _ = GdipDeleteBrush(brush.cast::<GpBrush>());
            return;
        }
    }
    let brush = solid_brush(color);
    let old = SelectObject(hdc, brush.into());
    let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
    let _ =
        windows::Win32::Graphics::Gdi::Ellipse(hdc, rect.left, rect.top, rect.right, rect.bottom);
    SelectObject(hdc, old_pen);
    SelectObject(hdc, old);
    let _ = DeleteObject(brush.into());
}

unsafe fn slider_value(index: usize) -> f32 {
    let style = STYLE.unwrap();
    match index {
        0 => style.displacement_scale / 200.0,
        1 => style.blur_amount,
        2 => (style.saturation - 1.0) / 2.0,
        3 => style.chromatic_offset / 20.0,
        4 => style.elasticity,
        5 => (style.corner_radius.min(100.0)) / 100.0,
        _ => 0.0,
    }
    .clamp(0.0, 1.0)
}

unsafe fn set_slider(index: usize, x: i32) {
    let Some(mut style) = STYLE else { return };
    let left = if index % 2 == 0 { 24 } else { 218 };
    let value = ((x - left) as f32 / 178.0).clamp(0.0, 1.0);
    match index {
        0 => style.displacement_scale = (value * 200.0).round(),
        1 => style.blur_amount = value,
        2 => style.saturation = 1.0 + value * 2.0,
        3 => style.chromatic_offset = (value * 20.0).round(),
        4 => style.elasticity = value,
        5 => style.corner_radius = if value > 0.98 { 999.0 } else { value * 100.0 },
        _ => return,
    }
    STYLE = Some(style);
    update_live_style(style);
}

unsafe fn slider_label(index: usize) -> (&'static str, String) {
    let style = STYLE.unwrap();
    match index {
        0 => (
            "Displacement scale",
            format!("{:.0}", style.displacement_scale),
        ),
        1 => ("Blur amount", format!("{:.1}", style.blur_amount)),
        2 => ("Saturation", format!("{:.0}%", style.saturation * 100.0)),
        3 => ("Chromatic aberr.", format!("{:.1}", style.chromatic_offset)),
        4 => ("Elasticity", format!("{:.2}", style.elasticity)),
        5 => (
            "Corner radius",
            if style.corner_radius > 100.0 {
                "Full".into()
            } else {
                format!("{:.0}px", style.corner_radius)
            },
        ),
        _ => ("", String::new()),
    }
}

unsafe fn paint_slider(hdc: windows::Win32::Graphics::Gdi::HDC, index: usize) {
    let y = slider_y(index);
    let left = if index % 2 == 0 { 24 } else { 218 };
    let right = left + 178;
    let (label, value) = slider_label(index);
    select_font(hdc, FONT_LABEL);
    text(
        hdc,
        label,
        RECT {
            left,
            top: y - 25,
            right: right - 54,
            bottom: y - 5,
        },
        rgb(220, 225, 232),
        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
    );
    select_font(hdc, FONT_LABEL);
    text(
        hdc,
        &value,
        RECT {
            left: right - 54,
            top: y - 25,
            right,
            bottom: y - 5,
        },
        rgb(112, 190, 255),
        DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
    );
    fill_rect(
        hdc,
        RECT {
            left,
            top: y,
            right,
            bottom: y + 6,
        },
        rgb(65, 70, 80),
    );
    let knob_x = left + (slider_value(index) * 178.0) as i32;
    fill_rect(
        hdc,
        RECT {
            left,
            top: y,
            right: knob_x.max(30),
            bottom: y + 6,
        },
        rgb(64, 151, 238),
    );
    fill_ellipse(
        hdc,
        RECT {
            left: knob_x - 6,
            top: y - 4,
            right: knob_x + 7,
            bottom: y + 9,
        },
        rgb(235, 245, 255),
    );
}

unsafe fn paint_panel(hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    let target_hdc = BeginPaint(hwnd, &mut paint);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let mut buffer_hdc = windows::Win32::Graphics::Gdi::HDC::default();
    let buffer = BeginBufferedPaint(
        target_hdc,
        &client,
        BPBF_COMPATIBLEBITMAP,
        None,
        &mut buffer_hdc,
    );
    let hdc = if buffer != 0 { buffer_hdc } else { target_hdc };
    PAINT_SCALE = client.right as f32 / 420.0;
    begin_antialiased_geometry(hdc);
    // Black is the layered-window color key; only controls and text remain over the D3D glass.
    let background = solid_brush(COLORREF(0));
    FillRect(
        hdc,
        &RECT {
            left: 0,
            top: 0,
            right: client.right,
            bottom: client.bottom,
        },
        background,
    );
    let _ = DeleteObject(background.into());
    SetBkMode(hdc, TRANSPARENT);
    for (top, knob_x) in [(18, 30), (25, 37), (32, 26)] {
        fill_round_rect(
            hdc,
            RECT {
                left: 22,
                top,
                right: 42,
                bottom: top + 2,
            },
            2,
            rgb(142, 153, 166),
        );
        fill_round_rect(
            hdc,
            RECT {
                left: knob_x,
                top: top - 2,
                right: knob_x + 4,
                bottom: top + 4,
            },
            4,
            rgb(226, 234, 242),
        );
    }
    select_font(hdc, FONT_LABEL);
    text(
        hdc,
        "Glass controls",
        RECT {
            left: 54,
            top: 0,
            right: 330,
            bottom: PANEL_COLLAPSED_H,
        },
        rgb(242, 245, 248),
        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
    );
    let pen = CreatePen(PS_SOLID, scaled(2).max(1), rgb(170, 181, 193));
    let old_pen = SelectObject(hdc, pen.into());
    if EXPANDED {
        let _ = MoveToEx(hdc, scaled(374), scaled(29), None);
        let _ = LineTo(hdc, scaled(381), scaled(22));
        let _ = LineTo(hdc, scaled(388), scaled(29));
    } else {
        let _ = MoveToEx(hdc, scaled(374), scaled(22), None);
        let _ = LineTo(hdc, scaled(381), scaled(29));
        let _ = LineTo(hdc, scaled(388), scaled(22));
    }
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(pen.into());
    if EXPANDED {
        select_font(hdc, FONT_BODY);
        text(
            hdc,
            "Refraction mode",
            RECT {
                left: 24,
                top: 58,
                right: 396,
                bottom: 80,
            },
            rgb(220, 225, 232),
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        let modes = ["Standard", "Polar", "Prominent", "Shader"];
        let selected = STYLE.unwrap().refraction_mode as usize;
        for (index, mode) in modes.iter().enumerate() {
            let left = 24 + index as i32 * 93;
            fill_round_rect(
                hdc,
                RECT {
                    left,
                    top: 80,
                    right: left + 87,
                    bottom: 112,
                },
                6,
                if index == selected {
                    rgb(48, 118, 187)
                } else {
                    rgb(45, 49, 57)
                },
            );
            fixed_text(
                hdc,
                mode,
                RECT {
                    left,
                    top: 80,
                    right: left + 87,
                    bottom: 112,
                },
                rgb(240, 243, 247),
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
        }
        for index in 0..6 {
            paint_slider(hdc, index);
        }
        let style = STYLE.unwrap();
        let box_color = if style.over_light {
            rgb(64, 151, 238)
        } else {
            rgb(55, 60, 69)
        };
        fill_round_rect(
            hdc,
            RECT {
                left: 24,
                top: footer_top(),
                right: 45,
                bottom: footer_top() + 21,
            },
            4,
            box_color,
        );
        if style.over_light {
            fixed_text(
                hdc,
                "x",
                RECT {
                    left: 24,
                    top: footer_top() - 1,
                    right: 45,
                    bottom: footer_top() + 21,
                },
                rgb(255, 255, 255),
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
        }
        text(
            hdc,
            "Over light",
            RECT {
                left: 53,
                top: footer_top() - 6,
                right: 190,
                bottom: footer_top() + FOOTER_HEIGHT,
            },
            rgb(228, 232, 238),
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        text(
            hdc,
            "Darken glass on bright backgrounds",
            RECT {
                left: 142,
                top: footer_top() - 6,
                right: 400,
                bottom: footer_top() + FOOTER_HEIGHT,
            },
            rgb(135, 145, 158),
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
    }
    end_antialiased_geometry();
    if buffer != 0 {
        let _ = EndBufferedPaint(buffer, true);
    }
    let _ = EndPaint(hwnd, &paint);
}

unsafe extern "system" fn panel_proc(
    hwnd: HWND,
    msg: u32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let coordinate_scale = 420.0 / client.right.max(1) as f32;
    let x = (lparam.0 as i16 as f32 * coordinate_scale).round() as i32;
    let y = ((lparam.0 >> 16) as i16 as f32 * coordinate_scale).round() as i32;
    match msg {
        WM_LBUTTONDOWN => {
            if y < PANEL_COLLAPSED_H {
                EXPANDED = !EXPANDED;
                let _ = InvalidateRect(PANEL_WINDOW, None, false);
                return LRESULT(0);
            }
            if EXPANDED && (80..=112).contains(&y) {
                let mode = ((x - 24) / 93).clamp(0, 3) as f32;
                let mut style = STYLE.unwrap();
                style.refraction_mode = mode;
                STYLE = Some(style);
                update_live_style(style);
                let _ = InvalidateRect(PANEL_WINDOW, None, false);
                return LRESULT(0);
            }
            if EXPANDED && (footer_top() - 8..=expanded_height()).contains(&y) {
                let mut style = STYLE.unwrap();
                style.over_light = !style.over_light;
                STYLE = Some(style);
                update_live_style(style);
                let _ = InvalidateRect(PANEL_WINDOW, None, false);
                return LRESULT(0);
            }
            if EXPANDED {
                for index in 0..PARAMETER_COUNT {
                    let left = if index % 2 == 0 { 16 } else { 210 };
                    let right = left + 194;
                    let track_y = slider_y(index);
                    if (track_y - 12..=track_y + 16).contains(&y) && (left..=right).contains(&x) {
                        ACTIVE_SLIDER = Some(index);
                        let _ = SetCapture(hwnd);
                        set_slider(index, x);
                        let _ = InvalidateRect(PANEL_WINDOW, None, false);
                        break;
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE if ACTIVE_SLIDER.is_some() => {
            set_slider(ACTIVE_SLIDER.unwrap(), x);
            let _ = InvalidateRect(PANEL_WINDOW, None, false);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            ACTIVE_SLIDER = None;
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, _wparam, lparam),
    }
}

unsafe extern "system" fn panel_visual_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_PAINT => {
            paint_panel(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn content_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let target_hdc = BeginPaint(hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let mut buffer_hdc = windows::Win32::Graphics::Gdi::HDC::default();
            let buffer = BeginBufferedPaint(
                target_hdc,
                &client,
                BPBF_COMPATIBLEBITMAP,
                None,
                &mut buffer_hdc,
            );
            let hdc = if buffer != 0 { buffer_hdc } else { target_hdc };
            PAINT_SCALE = client.right as f32 / 420.0;
            begin_antialiased_geometry(hdc);
            let black = solid_brush(COLORREF(0));
            FillRect(
                hdc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: client.right,
                    bottom: client.bottom,
                },
                black,
            );
            let _ = DeleteObject(black.into());
            SetBkMode(hdc, TRANSPARENT);
            select_font(hdc, FONT_LABEL);
            select_font(hdc, FONT_TITLE);
            text(
                hdc,
                "User Info",
                RECT {
                    left: 42,
                    top: 36,
                    right: 360,
                    bottom: 62,
                },
                rgb(255, 255, 255),
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );
            select_font(hdc, FONT_DISPLAY);
            fixed_text(
                hdc,
                "JD",
                RECT {
                    left: 42,
                    top: 78,
                    right: 106,
                    bottom: 142,
                },
                rgb(255, 255, 255),
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
            text(
                hdc,
                "John Doe",
                RECT {
                    left: 126,
                    top: 78,
                    right: 360,
                    bottom: 108,
                },
                rgb(255, 255, 255),
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );
            select_font(hdc, FONT_BODY);
            text(
                hdc,
                "Software Engineer",
                RECT {
                    left: 126,
                    top: 108,
                    right: 365,
                    bottom: 134,
                },
                rgb(255, 255, 255),
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );
            select_font(hdc, FONT_BODY);
            for (label, value, top) in [
                ("Email", "john.doe@example.com", 158),
                ("Location", "San Francisco, CA", 188),
                ("Joined", "March 2023", 216),
            ] {
                text(
                    hdc,
                    label,
                    RECT {
                        left: 42,
                        top,
                        right: 126,
                        bottom: top + 26,
                    },
                    rgb(255, 255, 255),
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER,
                );
                text(
                    hdc,
                    value,
                    RECT {
                        left: 146,
                        top,
                        right: 378,
                        bottom: top + 26,
                    },
                    rgb(255, 255, 255),
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER,
                );
            }
            end_antialiased_geometry();
            if buffer != 0 {
                let _ = EndBufferedPaint(buffer, true);
            }
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub unsafe fn create_demo_windows(
    instance: HINSTANCE,
    lens_position: POINT,
    lens_size: POINT,
    style: EffectStyle,
) -> Result<(HWND, HWND, HWND)> {
    BufferedPaintInit()?;
    if GDIPLUS_TOKEN == 0 {
        let startup = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let _ = GdiplusStartup(&mut GDIPLUS_TOKEN, &startup, std::ptr::null_mut());
    }
    TEXT_THEME = Some(OpenThemeData(None, w!("WINDOW")));
    STYLE = Some(style);
    PANEL_WIDTH = lens_size.x;
    UI_SCALE = lens_size.x as f32 / 420.0;
    let font_scale = lens_size.x as f32 / 352.0;
    PANEL_HEIGHT = PANEL_COLLAPSED_H as f32 * UI_SCALE;
    FONT_BODY = Some(make_font((14.0 * font_scale).round() as i32, 500));
    FONT_LABEL = Some(make_font((14.0 * font_scale).round() as i32, 500));
    FONT_TITLE = Some(make_font(
        (20.0 * font_scale).round() as i32,
        FW_SEMIBOLD.0 as i32,
    ));
    FONT_DISPLAY = Some(make_font(
        (20.0 * font_scale).round() as i32,
        FW_SEMIBOLD.0 as i32,
    ));
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(content_proc),
        hInstance: instance,
        lpszClassName: w!("LiquidGlassDemoContent"),
        ..Default::default()
    });
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(panel_visual_proc),
        hInstance: instance,
        lpszClassName: w!("LiquidGlassInlinePanel"),
        ..Default::default()
    });
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(panel_proc),
        hInstance: instance,
        hCursor: LoadCursorW(None, IDC_HAND)?,
        lpszClassName: w!("LiquidGlassPanelInput"),
        ..Default::default()
    });
    let content = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
        w!("LiquidGlassDemoContent"),
        w!(""),
        WS_POPUP | WS_VISIBLE,
        lens_position.x,
        lens_position.y,
        lens_size.x,
        lens_size.y,
        None,
        None,
        Some(instance),
        None,
    )?;
    SetLayeredWindowAttributes(content, COLORREF(0), 255, LWA_COLORKEY)?;
    let panel = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        w!("LiquidGlassInlinePanel"),
        w!(""),
        WS_POPUP | WS_VISIBLE,
        lens_position.x,
        lens_position.y + lens_size.y - (PANEL_COLLAPSED_H as f32 * UI_SCALE).round() as i32,
        lens_size.x,
        (PANEL_COLLAPSED_H as f32 * UI_SCALE).round() as i32,
        None,
        None,
        Some(instance),
        None,
    )?;
    SetLayeredWindowAttributes(panel, COLORREF(0), 255, LWA_COLORKEY)?;
    let region = windows::Win32::Graphics::Gdi::CreateRoundRectRgn(
        0,
        0,
        lens_size.x + 1,
        (PANEL_COLLAPSED_H as f32 * UI_SCALE).round() as i32 + 1,
        (16.0 * UI_SCALE).round() as i32,
        (16.0 * UI_SCALE).round() as i32,
    );
    SetWindowRgn(panel, Some(region), true);
    PANEL_WINDOW = Some(panel);
    let input = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        w!("LiquidGlassPanelInput"),
        w!(""),
        WS_POPUP | WS_VISIBLE,
        lens_position.x,
        lens_position.y + lens_size.y - (PANEL_COLLAPSED_H as f32 * UI_SCALE).round() as i32,
        lens_size.x,
        (64.0 * UI_SCALE).round() as i32,
        None,
        None,
        Some(instance),
        None,
    )?;
    SetLayeredWindowAttributes(input, COLORREF(0), 1, LWA_ALPHA)?;
    INPUT_WINDOW = Some(input);
    LAST_TICK = Some(Instant::now());
    Ok((content, panel, input))
}
