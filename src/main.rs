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

const LENS_W: i32 = 420;
const LENS_H: i32 = 280;

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
}

const SHADER: &str = r#"
cbuffer C : register(b0) { float2 outputOrigin, outputSize, windowOrigin, windowSize, lensSize, padding; };
Texture2D desktop : register(t0); SamplerState samp : register(s0);
struct V { float4 pos:SV_POSITION; float2 uv:TEXCOORD0; };
V VSMain(uint id:SV_VertexID) { V o; float2 p=float2((id<<1)&2,id&2); o.uv=p; o.pos=float4(p*float2(2,-2)+float2(-1,1),0,1); return o; }
float sdf(float2 p,float2 halfSize,float radius) { float2 q=abs(p)-halfSize+radius; return min(max(q.x,q.y),0)+length(max(q,0))-radius; }
float3 blur9(float2 uv,float2 t,float r) {
 float3 c=desktop.Sample(samp,uv).rgb*.24;
 c+=(desktop.Sample(samp,uv+t*float2(r,0)).rgb+desktop.Sample(samp,uv-t*float2(r,0)).rgb+desktop.Sample(samp,uv+t*float2(0,r)).rgb+desktop.Sample(samp,uv-t*float2(0,r)).rgb)*.12;
 c+=(desktop.Sample(samp,uv+t*float2(r,r)).rgb+desktop.Sample(samp,uv+t*float2(-r,r)).rgb+desktop.Sample(samp,uv+t*float2(r,-r)).rgb+desktop.Sample(samp,uv-t*float2(r,r)).rgb)*.07; return c;
}
float4 PSMain(V i):SV_TARGET {
 float2 px=i.uv*windowSize, local=px-padding, p=local-lensSize*.5; float radius=lensSize.y*.5;
 float d=sdf(p,lensSize*.5,radius); float sd=sdf(p-float2(0,7),lensSize*.5,radius);
 float shadow=exp(-pow(max(sd,0)/9,2))*.28; if(d>1.5) return float4(0,0,0,shadow);
 float2 uv=local/lensSize, center=uv-.5;
 float mapDistance=sdf(center,float2(.3,.2),.6);
 float displacement=smoothstep(.8,0,mapDistance-.15);
 float scaled=smoothstep(0,1,displacement);
 float2 source=(center*scaled+.5)*lensSize;
 float2 screenUv=(windowOrigin+padding+source-outputOrigin)/outputSize, texel=1/outputSize;
 float edge=saturate(1-smoothstep(0,24,-d));
 float2 radial=normalize(center+float2(.0001,.0001));
 float3 col=desktop.Sample(samp,screenUv).rgb;
 float chroma=edge*edge*.8;
 col.r=desktop.Sample(samp,screenUv+radial*texel*chroma).r;
 col.b=desktop.Sample(samp,screenUv-radial*texel*chroma).b;
 col=saturate((col-.5)*1.04+.5); col=saturate(col*1.025+.018);
 float topLight=edge*saturate(.5-uv.y)*.07;
 float lowerShadow=edge*saturate(uv.y-.34)*.08;
 col=saturate(col+topLight-lowerShadow);
 float a=smoothstep(1.5,-1,d); return float4(col*a,a);
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
    duplication: Option<IDXGIOutputDuplication>,
    desktop_texture: Option<ID3D11Texture2D>,
    desktop_view: Option<ID3D11ShaderResourceView>,
    output_rect: RECT,
    width: i32,
    height: i32,
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
        let mut renderer = Self {
            device,
            context,
            swap_chain,
            target: target.unwrap(),
            vertex_shader: vertex_shader.unwrap(),
            pixel_shader: pixel_shader.unwrap(),
            constants: constants.unwrap(),
            sampler: sampler.unwrap(),
            duplication: None,
            desktop_texture: None,
            desktop_view: None,
            output_rect: RECT::default(),
            width,
            height,
        };
        renderer.create_duplication(hwnd)?;
        Ok(renderer)
    }

    unsafe fn create_duplication(&mut self, hwnd: HWND) -> Result<()> {
        let dxgi: IDXGIDevice = self.device.cast()?;
        let adapter = dxgi.GetAdapter()?;
        let mut window = RECT::default();
        GetWindowRect(hwnd, &mut window)?;
        let center = POINT {
            x: (window.left + window.right) / 2,
            y: (window.top + window.bottom) / 2,
        };
        let mut selected = None;
        let mut index = 0;
        while let Ok(output) = adapter.EnumOutputs(index) {
            let desc = output.GetDesc()?;
            let r = desc.DesktopCoordinates;
            if selected.is_none()
                || (center.x >= r.left
                    && center.x < r.right
                    && center.y >= r.top
                    && center.y < r.bottom)
            {
                self.output_rect = r;
                selected = Some(output);
                if center.x >= r.left
                    && center.x < r.right
                    && center.y >= r.top
                    && center.y < r.bottom
                {
                    break;
                }
            }
            index += 1;
        }
        let output1: IDXGIOutput1 = selected
            .ok_or_else(windows::core::Error::from_thread)?
            .cast()?;
        self.duplication = Some(output1.DuplicateOutput(&self.device)?);
        Ok(())
    }

    unsafe fn capture(&mut self) {
        let Some(duplication) = self.duplication.clone() else {
            return;
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        if duplication
            .AcquireNextFrame(
                if self.desktop_texture.is_some() {
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
                let recreate = self.desktop_texture.as_ref().is_none_or(|texture| {
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
                    if self
                        .device
                        .CreateTexture2D(&desc, None, Some(&mut texture))
                        .is_ok()
                    {
                        self.desktop_texture = texture;
                        let mut view = None;
                        if self
                            .device
                            .CreateShaderResourceView(
                                self.desktop_texture.as_ref().unwrap(),
                                None,
                                Some(&mut view),
                            )
                            .is_ok()
                        {
                            self.desktop_view = view;
                        }
                    }
                }
                if let Some(texture) = &self.desktop_texture {
                    self.context.CopyResource(texture, &frame);
                }
            }
        }
        let _ = duplication.ReleaseFrame();
    }

    unsafe fn render(&mut self, hwnd: HWND) {
        self.capture();
        let Some(view) = self.desktop_view.clone() else {
            return;
        };
        let mut window = RECT::default();
        if GetWindowRect(hwnd, &mut window).is_err() {
            return;
        }
        let constants = Constants {
            output_origin: [self.output_rect.left as f32, self.output_rect.top as f32],
            output_size: [
                (self.output_rect.right - self.output_rect.left) as f32,
                (self.output_rect.bottom - self.output_rect.top) as f32,
            ],
            window_origin: [window.left as f32, window.top as f32],
            window_size: [self.width as f32, self.height as f32],
            lens_size: [LENS_W as f32, LENS_H as f32],
            padding: [LENS_POSITION.x as f32, LENS_POSITION.y as f32],
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
            .is_ok()
        {
            mapped.pData.cast::<Constants>().write(constants);
            self.context.Unmap(&self.constants, 0);
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
        self.context.IASetInputLayout(None);
        self.context
            .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.context.VSSetShader(&self.vertex_shader, None);
        self.context.PSSetShader(&self.pixel_shader, None);
        self.context
            .PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
        self.context
            .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        self.context.PSSetShaderResources(0, Some(&[Some(view)]));
        self.context.Draw(3, 0);
        self.context.PSSetShaderResources(0, Some(&[None]));
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
                let x = (DRAG_LENS.x + cursor.x - DRAG_CURSOR.x)
                    .clamp(0, (GetSystemMetrics(SM_CXSCREEN) - LENS_W).max(0));
                let y = (DRAG_LENS.y + cursor.y - DRAG_CURSOR.y)
                    .clamp(0, (GetSystemMetrics(SM_CYSCREEN) - LENS_H).max(0));
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
        let screen_width = GetSystemMetrics(SM_CXSCREEN);
        let screen_height = GetSystemMetrics(SM_CYSCREEN);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
            name,
            w!("Liquid Glass"),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
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
