#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::c_void,
    mem::size_of,
    ptr::null_mut,
    sync::{Arc, Mutex, OnceLock},
};
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM},
        Graphics::Gdi::{
            AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
            CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
            HBITMAP, HGDIOBJ, ReleaseDC, SelectObject,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            Input::KeyboardAndMouse::{ReleaseCapture, VK_ESCAPE},
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, GetWindowRect,
                HTCAPTION, HTCLIENT, HTTRANSPARENT, IDC_ARROW, LoadCursorW, MSG, PostQuitMessage,
                RegisterClassW, SW_SHOW, SendMessageW, SetTimer, SetWindowDisplayAffinity,
                SetWindowLongPtrW, ShowWindow, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow,
                WDA_EXCLUDEFROMCAPTURE, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOVE,
                WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
            },
        },
    },
    core::{Result, w},
};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

const LENS_W: i32 = 420;
const LENS_H: i32 = 280;
const PAD: i32 = 18;
const WIDTH: i32 = LENS_W + PAD * 2;
const HEIGHT: i32 = LENS_H + PAD * 2;

#[derive(Default)]
struct ScreenFrame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    sequence: u64,
}

static SCREEN_FRAME: OnceLock<Arc<Mutex<ScreenFrame>>> = OnceLock::new();

struct ScreenCapture {
    shared: Arc<Mutex<ScreenFrame>>,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for ScreenCapture {
    type Flags = Arc<Mutex<ScreenFrame>>;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            shared: ctx.flags,
            scratch: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> std::result::Result<(), Self::Error> {
        let buffer = frame.buffer()?;
        let width = buffer.width();
        let height = buffer.height();
        let bytes = buffer.as_nopadding_buffer(&mut self.scratch);
        let mut shared = self.shared.lock().unwrap();
        shared.pixels.clear();
        shared.pixels.extend_from_slice(bytes);
        shared.width = width;
        shared.height = height;
        shared.sequence = shared.sequence.wrapping_add(1);
        Ok(())
    }
}

struct Renderer {
    dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    pixels: *mut u32,
    last_sequence: u64,
}

impl Renderer {
    unsafe fn new() -> Result<Box<Self>> {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let _ = ReleaseDC(None, screen);
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: WIDTH,
            biHeight: -HEIGHT,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let old_bitmap = SelectObject(dc, bitmap.into());
        Ok(Box::new(Self {
            dc,
            bitmap,
            old_bitmap,
            pixels: bits.cast(),
            last_sequence: 0,
        }))
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old_bitmap);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

fn smooth_step(a: f32, b: f32, value: f32) -> f32 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rounded_rect_sdf(x: f32, y: f32, width: f32, height: f32, radius: f32) -> f32 {
    let qx = x.abs() - width + radius;
    let qy = y.abs() - height + radius;
    qx.max(qy).min(0.0) + qx.max(0.0).hypot(qy.max(0.0)) - radius
}

unsafe fn renderer(hwnd: HWND) -> Option<&'static mut Renderer> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Renderer).as_mut()
}

unsafe fn render(hwnd: HWND) {
    let mut rect = Default::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return;
    }
    let Some(renderer) = renderer(hwnd) else {
        return;
    };
    let Some(shared) = SCREEN_FRAME.get() else {
        return;
    };
    let frame = shared.lock().unwrap();
    if frame.pixels.is_empty() || frame.sequence == renderer.last_sequence {
        return;
    }
    renderer.last_sequence = frame.sequence;

    let count = (WIDTH * HEIGHT) as usize;
    let pixels = std::slice::from_raw_parts_mut(renderer.pixels, count);
    pixels.fill(0);

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let lx = x - PAD;
            let ly = y - PAD;
            let uvx = lx as f32 / LENS_W as f32;
            let uvy = ly as f32 / LENS_H as f32;
            let ix = uvx - 0.5;
            let iy = uvy - 0.5;

            // Exact displacement function used by liquid-glass.js.
            let distance = rounded_rect_sdf(ix, iy, 0.3, 0.2, 0.6);
            let displacement = smooth_step(0.8, 0.0, distance - 0.15);
            let scaled = smooth_step(0.0, 1.0, displacement);
            let sx =
                ((ix * scaled + 0.5) * LENS_W as f32).clamp(0.0, (LENS_W - 1) as f32) as i32 + PAD;
            let sy =
                ((iy * scaled + 0.5) * LENS_H as f32).clamp(0.0, (LENS_H - 1) as f32) as i32 + PAD;

            let pixel_distance = rounded_rect_sdf(
                lx as f32 - LENS_W as f32 / 2.0,
                ly as f32 - LENS_H as f32 / 2.0,
                LENS_W as f32 / 2.0,
                LENS_H as f32 / 2.0,
                LENS_H as f32 / 2.0,
            );
            let alpha = smooth_step(1.5, -1.5, pixel_distance);
            if alpha <= 0.0 {
                let shadow_distance = rounded_rect_sdf(
                    lx as f32 - LENS_W as f32 / 2.0,
                    ly as f32 - LENS_H as f32 / 2.0 - 6.0,
                    LENS_W as f32 / 2.0,
                    LENS_H as f32 / 2.0,
                    LENS_H as f32 / 2.0,
                );
                if shadow_distance < 18.0 {
                    let falloff = (-shadow_distance.max(0.0).powi(2) / 90.0).exp();
                    pixels[(y * WIDTH + x) as usize] = ((54.0 * falloff) as u32) << 24;
                }
                continue;
            }

            let screen_x = rect.left + sx;
            let screen_y = rect.top + sy;
            if screen_x < 0
                || screen_y < 0
                || screen_x >= frame.width as i32
                || screen_y >= frame.height as i32
            {
                continue;
            }
            let source_index = ((screen_y as u32 * frame.width + screen_x as u32) * 4) as usize;
            let b = frame.pixels[source_index] as f32;
            let g = frame.pixels[source_index + 1] as f32;
            let r = frame.pixels[source_index + 2] as f32;
            let edge = (pixel_distance / 18.0).exp().clamp(0.0, 1.0);
            let lower_edge = smooth_step(-0.15, 0.5, iy);
            let darken = edge * lower_edge * 0.13;
            let highlight = edge * (1.0 - lower_edge) * 0.075;
            let grade =
                |v: f32| ((v * 0.965 + 9.0) * (1.0 - darken) + 255.0 * highlight).clamp(0.0, 255.0);
            let a = (alpha * 255.0) as u32;
            let premul = |v: f32| (grade(v) * a as f32 / 255.0) as u32;
            pixels[(y * WIDTH + x) as usize] =
                a << 24 | premul(r) << 16 | premul(g) << 8 | premul(b);
        }
    }

    let size = SIZE {
        cx: WIDTH,
        cy: HEIGHT,
    };
    let source = POINT::default();
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = UpdateLayeredWindow(
        hwnd,
        None,
        None,
        Some(&size),
        Some(renderer.dc),
        Some(&source),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            match Renderer::new() {
                Ok(value) => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(value) as isize);
                }
                Err(_) => return LRESULT(-1),
            }
            let _ = SetTimer(Some(hwnd), 1, 16, None);
            render(hwnd);
            LRESULT(0)
        }
        WM_MOVE | WM_TIMER => {
            render(hwnd);
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let mut rect = Default::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return LRESULT(HTTRANSPARENT as isize);
            }
            let x = lparam.0 as i16 as i32 - rect.left - PAD;
            let y = (lparam.0 >> 16) as i16 as i32 - rect.top - PAD;
            let d = rounded_rect_sdf(
                x as f32 - LENS_W as f32 / 2.0,
                y as f32 - LENS_H as f32 / 2.0,
                LENS_W as f32 / 2.0,
                LENS_H as f32 / 2.0,
                LENS_H as f32 / 2.0,
            );
            LRESULT(if d <= 0.0 {
                HTCLIENT as isize
            } else {
                HTTRANSPARENT as isize
            })
        }
        WM_LBUTTONDOWN => {
            let _ = ReleaseCapture();
            SendMessageW(
                hwnd,
                WM_NCLBUTTONDOWN,
                Some(WPARAM(HTCAPTION as usize)),
                Some(LPARAM(0)),
            );
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            DestroyWindow(hwnd).ok();
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Renderer;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let shared = Arc::new(Mutex::new(ScreenFrame::default()));
        let _ = SCREEN_FRAME.set(shared.clone());
        let capture_settings = Settings::new(
            Monitor::primary()?,
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Exclude,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            shared,
        );
        let capture = ScreenCapture::start_free_threaded(capture_settings)?;
        let module = GetModuleHandleW(None)?;
        let class_name = w!("LiquidGlassShader");
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: HINSTANCE(module.0),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            class_name,
            w!("Liquid Glass"),
            WS_POPUP | WS_VISIBLE,
            400,
            240,
            WIDTH,
            HEIGHT,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?;
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        capture.stop()?;
    }
    Ok(())
}
