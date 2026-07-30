#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn, static_mut_refs)]

use std::{mem::size_of, slice};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
                D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::D3DCompile,
                ID3DBlob,
            },
            Direct3D11::*,
            Dwm::DwmExtendFrameIntoClientArea,
            Dxgi::{Common::*, *},
            Gdi::{CreateRoundRectRgn, SetWindowRgn},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::MARGINS,
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            Input::KeyboardAndMouse::{
                MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture, VK_ESCAPE,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{Interface, Result, s, w},
};

// Lens window size in physical pixels.
const LENS_W: i32 = 420;
const LENS_H: i32 = 280;

// Shadow: vertical offset and softness are in physical pixels.
const SHADOW_OFFSET_Y: f32 = 7.0;
const SHADOW_SOFTNESS: f32 = 9.0;
const SHADOW_OPACITY: f32 = 0.28;

// Refraction: smaller core values or a larger bias strengthen edge distortion.
const REFRACTION_CORE_X: f32 = 0.30;
const REFRACTION_CORE_Y: f32 = 0.20;
const REFRACTION_RADIUS: f32 = 0.60;
const REFRACTION_TRANSITION: f32 = 0.80;
const REFRACTION_BIAS: f32 = 0.15;

// Edge effect width and RGB channel separation, in physical pixels.
const EDGE_EFFECT_WIDTH: f32 = 24.0;
const CHROMATIC_OFFSET: f32 = 0.80;

// Color: contrast around 0.5, followed by gain and additive lift.
const COLOR_CONTRAST: f32 = 1.04;
const COLOR_GAIN: f32 = 1.025;
const COLOR_LIFT: f32 = 0.018;

// Directional edge lighting.
const TOP_LIGHT_POSITION: f32 = 0.50;
const TOP_LIGHT_STRENGTH: f32 = 0.07;
const LOWER_SHADOW_POSITION: f32 = 0.34;
const LOWER_SHADOW_STRENGTH: f32 = 0.08;

// Outline anti-aliasing. Increase both slightly for a softer edge.
const AA_DERIVATIVE_SCALE: f32 = 0.75;
const AA_MIN_HALF_WIDTH: f32 = 0.75;

static mut DRAGGING: bool = false;
static mut DRAG_CURSOR: POINT = POINT { x: 0, y: 0 };
static mut DRAG_LENS: POINT = POINT { x: 0, y: 0 };
static mut LENS_POSITION: POINT = POINT { x: 420, y: 240 };

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    output_origin: [f32; 2],
    output_size: [f32; 2],
    window_origin: [f32; 2],
    window_size: [f32; 2],
    lens_size: [f32; 2],
    padding: [f32; 2],
    shadow_settings: [f32; 4],
    refraction_settings: [f32; 4],
    effects_settings: [f32; 4],
    color_settings: [f32; 4],
    detail_settings: [f32; 4],
}

const SHADER: &str = r#"
cbuffer C : register(b0) {
 float2 outputOrigin, outputSize, windowOrigin, windowSize, lensSize, padding;
 float4 shadowSettings, refractionSettings, effectsSettings, colorSettings, detailSettings;
};
Texture2D desktop : register(t0); SamplerState samp : register(s0);
struct V { float4 pos:SV_POSITION; float2 uv:TEXCOORD0; };
V VSMain(uint id:SV_VertexID) { V o; float2 p=float2((id<<1)&2,id&2); o.uv=p; o.pos=float4(p*float2(2,-2)+float2(-1,1),0,1); return o; }
float sdf(float2 p,float2 halfSize,float radius) { float2 q=abs(p)-halfSize+radius; return min(max(q.x,q.y),0)+length(max(q,0))-radius; }
float4 PSMain(V i):SV_TARGET {
 float2 px=i.uv*windowSize, local=px-padding, p=local-lensSize*.5; float radius=lensSize.y*.5;
 float d=sdf(p,lensSize*.5,radius); float sd=sdf(p-float2(0,shadowSettings.x),lensSize*.5,radius);
 float shadow=exp(-pow(max(sd,0)/shadowSettings.y,2))*shadowSettings.z;
 float2 uv=local/lensSize, center=uv-.5;
 float mapDistance=sdf(center,refractionSettings.xy,refractionSettings.z);
 float displacement=smoothstep(refractionSettings.w,0,mapDistance-effectsSettings.x);
 float scaled=smoothstep(0,1,displacement);
 float2 source=(center*scaled+.5)*lensSize;
 float2 screenUv=(windowOrigin+padding+source-outputOrigin)/outputSize, texel=1/outputSize;
 float edge=saturate(1-smoothstep(0,effectsSettings.y,-d));
 float2 radial=normalize(center+float2(.0001,.0001));
 float3 col=desktop.Sample(samp,screenUv).rgb;
 float chroma=edge*edge*effectsSettings.z;
 col.r=desktop.Sample(samp,screenUv+radial*texel*chroma).r;
 col.b=desktop.Sample(samp,screenUv-radial*texel*chroma).b;
 col=saturate((col-.5)*effectsSettings.w+.5); col=saturate(col*colorSettings.x+colorSettings.y);
 float topLight=edge*saturate(colorSettings.z-uv.y)*colorSettings.w;
 float lowerShadow=edge*saturate(uv.y-detailSettings.x)*detailSettings.y;
 col=saturate(col+topLight-lowerShadow);
 float aa=max(fwidth(d)*detailSettings.z,detailSettings.w);
 float glassAlpha=smoothstep(aa,-aa,d);
 float alpha=glassAlpha+shadow*(1-glassAlpha);
 return float4(col*glassAlpha,alpha);
}"#;

struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain,
    target: ID3D11RenderTargetView,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    constants: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
    outputs: Vec<OutputCapture>,
    width: i32,
    height: i32,
}

struct OutputCapture {
    duplication: IDXGIOutputDuplication,
    desktop_texture: Option<ID3D11Texture2D>,
    desktop_view: Option<ID3D11ShaderResourceView>,
    rect: RECT,
}

fn compile(entry: windows::core::PCSTR, target: windows::core::PCSTR) -> Result<ID3DBlob> {
    unsafe {
        let mut code = None;
        let mut errors: Option<ID3DBlob> = None;
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            s!("liquid_glass.hlsl"),
            None,
            None,
            entry,
            target,
            1 << 15,
            0,
            &mut code,
            Some(&mut errors),
        )?;
        Ok(code.unwrap())
    }
}

impl Renderer {
    unsafe fn new(hwnd: HWND, width: i32, height: i32) -> Result<Self> {
        let swap_desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: width as u32,
                Height: height as u32,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                RefreshRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                ..Default::default()
            },
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: hwnd,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
            ..Default::default()
        };
        let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0];
        let mut device = None;
        let mut context = None;
        let mut swap_chain = None;
        let mut chosen = D3D_FEATURE_LEVEL::default();
        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&swap_desc),
            Some(&mut swap_chain),
            Some(&mut device),
            Some(&mut chosen),
            Some(&mut context),
        )?;
        let device = device.unwrap();
        let context = context.unwrap();
        let swap_chain = swap_chain.unwrap();
        let back: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
        let mut target = None;
        device.CreateRenderTargetView(&back, None, Some(&mut target))?;
        let vs_blob = compile(s!("VSMain"), s!("vs_5_0"))?;
        let ps_blob = compile(s!("PSMain"), s!("ps_5_0"))?;
        let blob_bytes = |blob: &ID3DBlob| {
            slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize())
        };
        let mut vertex_shader = None;
        device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vertex_shader))?;
        let mut pixel_shader = None;
        device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut pixel_shader))?;
        let buffer_desc = D3D11_BUFFER_DESC {
            ByteWidth: size_of::<Constants>() as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut constants = None;
        device.CreateBuffer(&buffer_desc, None, Some(&mut constants))?;
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
        let mut rasterizer = None;
        device.CreateRasterizerState(
            &D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                ScissorEnable: true.into(),
                DepthClipEnable: true.into(),
                ..Default::default()
            },
            Some(&mut rasterizer),
        )?;
        let mut renderer = Self {
            device,
            context,
            swap_chain,
            target: target.unwrap(),
            vertex_shader: vertex_shader.unwrap(),
            pixel_shader: pixel_shader.unwrap(),
            constants: constants.unwrap(),
            sampler: sampler.unwrap(),
            rasterizer: rasterizer.unwrap(),
            outputs: Vec::new(),
            width,
            height,
        };
        renderer.create_duplications()?;
        Ok(renderer)
    }

    unsafe fn create_duplications(&mut self) -> Result<()> {
        let dxgi: IDXGIDevice = self.device.cast()?;
        let adapter = dxgi.GetAdapter()?;
        self.outputs.clear();
        let mut index = 0;
        while let Ok(output) = adapter.EnumOutputs(index) {
            let desc = output.GetDesc()?;
            let output1: IDXGIOutput1 = output.cast()?;
            self.outputs.push(OutputCapture {
                duplication: output1.DuplicateOutput(&self.device)?,
                desktop_texture: None,
                desktop_view: None,
                rect: desc.DesktopCoordinates,
            });
            index += 1;
        }
        if self.outputs.is_empty() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(())
    }

    unsafe fn capture_output(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        output: &mut OutputCapture,
    ) {
        let duplication = output.duplication.clone();
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        if duplication
            .AcquireNextFrame(
                if output.desktop_texture.is_some() {
                    0
                } else {
                    100
                },
                &mut info,
                &mut resource,
            )
            .is_err()
        {
            return;
        }
        if let Some(resource) = resource {
            if let Ok(frame) = resource.cast::<ID3D11Texture2D>() {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                frame.GetDesc(&mut desc);
                let recreate = output.desktop_texture.as_ref().is_none_or(|texture| {
                    let mut old = D3D11_TEXTURE2D_DESC::default();
                    texture.GetDesc(&mut old);
                    old.Width != desc.Width
                        || old.Height != desc.Height
                        || old.Format != desc.Format
                });
                if recreate {
                    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                    desc.CPUAccessFlags = 0;
                    desc.Usage = D3D11_USAGE_DEFAULT;
                    desc.MiscFlags = 0;
                    let mut texture = None;
                    if device
                        .CreateTexture2D(&desc, None, Some(&mut texture))
                        .is_ok()
                    {
                        output.desktop_texture = texture;
                        let mut view = None;
                        if device
                            .CreateShaderResourceView(
                                output.desktop_texture.as_ref().unwrap(),
                                None,
                                Some(&mut view),
                            )
                            .is_ok()
                        {
                            output.desktop_view = view;
                        }
                    }
                }
                if let Some(texture) = &output.desktop_texture {
                    context.CopyResource(texture, &frame);
                }
            }
        }
        let _ = duplication.ReleaseFrame();
    }

    unsafe fn render(&mut self, hwnd: HWND) {
        for output in &mut self.outputs {
            Self::capture_output(&self.device, &self.context, output);
        }
        let mut window = RECT::default();
        if GetWindowRect(hwnd, &mut window).is_err() {
            return;
        }
        self.context
            .OMSetRenderTargets(Some(&[Some(self.target.clone())]), None);
        self.context.ClearRenderTargetView(&self.target, &[0.0; 4]);
        self.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            Width: self.width as f32,
            Height: self.height as f32,
            MaxDepth: 1.0,
            ..Default::default()
        }]));
        self.context.RSSetState(&self.rasterizer);
        self.context.IASetInputLayout(None);
        self.context
            .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.context.VSSetShader(&self.vertex_shader, None);
        self.context.PSSetShader(&self.pixel_shader, None);
        self.context
            .PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
        self.context
            .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        for output in &self.outputs {
            let Some(view) = output.desktop_view.clone() else {
                continue;
            };
            let constants = Constants {
                output_origin: [output.rect.left as f32, output.rect.top as f32],
                output_size: [
                    (output.rect.right - output.rect.left) as f32,
                    (output.rect.bottom - output.rect.top) as f32,
                ],
                window_origin: [window.left as f32, window.top as f32],
                window_size: [self.width as f32, self.height as f32],
                lens_size: [LENS_W as f32, LENS_H as f32],
                padding: [
                    (LENS_POSITION.x - window.left) as f32,
                    (LENS_POSITION.y - window.top) as f32,
                ],
                shadow_settings: [SHADOW_OFFSET_Y, SHADOW_SOFTNESS, SHADOW_OPACITY, 0.0],
                refraction_settings: [
                    REFRACTION_CORE_X,
                    REFRACTION_CORE_Y,
                    REFRACTION_RADIUS,
                    REFRACTION_TRANSITION,
                ],
                effects_settings: [
                    REFRACTION_BIAS,
                    EDGE_EFFECT_WIDTH,
                    CHROMATIC_OFFSET,
                    COLOR_CONTRAST,
                ],
                color_settings: [
                    COLOR_GAIN,
                    COLOR_LIFT,
                    TOP_LIGHT_POSITION,
                    TOP_LIGHT_STRENGTH,
                ],
                detail_settings: [
                    LOWER_SHADOW_POSITION,
                    LOWER_SHADOW_STRENGTH,
                    AA_DERIVATIVE_SCALE,
                    AA_MIN_HALF_WIDTH,
                ],
            };
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if self
                .context
                .Map(
                    &self.constants,
                    0,
                    D3D11_MAP_WRITE_DISCARD,
                    0,
                    Some(&mut mapped),
                )
                .is_err()
            {
                continue;
            }
            mapped.pData.cast::<Constants>().write(constants);
            self.context.Unmap(&self.constants, 0);
            self.context.RSSetScissorRects(Some(&[RECT {
                left: (output.rect.left - window.left).max(0),
                top: (output.rect.top - window.top).max(0),
                right: (output.rect.right - window.left).min(self.width),
                bottom: (output.rect.bottom - window.top).min(self.height),
            }]));
            self.context.PSSetShaderResources(0, Some(&[Some(view)]));
            self.context.Draw(3, 0);
            self.context.PSSetShaderResources(0, Some(&[None]));
        }
        let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_HOTKEY => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn input_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => {
            DRAGGING = true;
            let _ = SetCapture(hwnd);
            let _ = GetCursorPos(&mut DRAG_CURSOR);
            DRAG_LENS = LENS_POSITION;
            LRESULT(0)
        }
        WM_MOUSEMOVE if DRAGGING => {
            let mut cursor = POINT::default();
            if GetCursorPos(&mut cursor).is_ok() {
                let virtual_left = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let virtual_top = GetSystemMetrics(SM_YVIRTUALSCREEN);
                let virtual_right = virtual_left + GetSystemMetrics(SM_CXVIRTUALSCREEN);
                let virtual_bottom = virtual_top + GetSystemMetrics(SM_CYVIRTUALSCREEN);
                let x = (DRAG_LENS.x + cursor.x - DRAG_CURSOR.x)
                    .clamp(virtual_left, (virtual_right - LENS_W).max(virtual_left));
                let y = (DRAG_LENS.y + cursor.y - DRAG_CURSOR.y)
                    .clamp(virtual_top, (virtual_bottom - LENS_H).max(virtual_top));
                if x != LENS_POSITION.x || y != LENS_POSITION.y {
                    LENS_POSITION = POINT { x, y };
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        x,
                        y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            DRAGGING = false;
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let module = GetModuleHandleW(None)?;
        let name = w!("RustLiquidGlassD3D11");
        let input_name = w!("RustLiquidGlassInput");
        RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_proc),
            hInstance: HINSTANCE(module.0),
            hCursor: LoadCursorW(None, IDC_HAND)?,
            lpszClassName: name,
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(input_proc),
            hInstance: HINSTANCE(module.0),
            hCursor: LoadCursorW(None, IDC_HAND)?,
            lpszClassName: input_name,
            ..Default::default()
        });
        let screen_left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let screen_top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let screen_width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let screen_height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
            name,
            w!("Liquid Glass"),
            WS_POPUP | WS_VISIBLE,
            screen_left,
            screen_top,
            screen_width,
            screen_height,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?;
        SetLayeredWindowAttributes(
            hwnd,
            windows::Win32::Foundation::COLORREF(0),
            255,
            LWA_ALPHA,
        )?;
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins)?;
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
        RegisterHotKey(Some(hwnd), 1, MOD_NOREPEAT, VK_ESCAPE.0 as u32)?;
        let input_hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            input_name,
            w!("Liquid Glass Input"),
            WS_POPUP | WS_VISIBLE,
            LENS_POSITION.x,
            LENS_POSITION.y,
            LENS_W,
            LENS_H,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?;
        SetLayeredWindowAttributes(
            input_hwnd,
            windows::Win32::Foundation::COLORREF(0),
            1,
            LWA_ALPHA,
        )?;
        let input_region = CreateRoundRectRgn(0, 0, LENS_W + 1, LENS_H + 1, LENS_H, LENS_H);
        SetWindowRgn(input_hwnd, Some(input_region), true);
        SetWindowDisplayAffinity(input_hwnd, WDA_EXCLUDEFROMCAPTURE)?;
        let mut renderer = Renderer::new(hwnd, screen_width, screen_height)?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = ShowWindow(input_hwnd, SW_SHOW);
        let mut msg = MSG::default();
        loop {
            let mut handled = 0;
            while handled < 32 && PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                handled += 1;
            }
            renderer.render(hwnd);
        }
    }
}
