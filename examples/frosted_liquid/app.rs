#![allow(unsafe_op_in_unsafe_fn, static_mut_refs)]

use std::{mem::size_of, slice};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Direct2D::Common::{
                D2D_MATRIX_5X4_F, D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE,
                D2D1_BLEND_MODE_SCREEN, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_COPY,
                D2D1_PIXEL_FORMAT,
            },
            Direct2D::{
                CLSID_D2D1Blend, CLSID_D2D1ColorMatrix, CLSID_D2D1DisplacementMap,
                CLSID_D2D1GaussianBlur, CLSID_D2D1Saturation, D2D1_BITMAP_OPTIONS_NONE,
                D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_BLEND_PROP_MODE,
                D2D1_CHANNEL_SELECTOR_B, D2D1_CHANNEL_SELECTOR_R,
                D2D1_COLORMATRIX_PROP_COLOR_MATRIX, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
                D2D1_DISPLACEMENTMAP_PROP_SCALE, D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT,
                D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT,
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_PROPERTY_TYPE_ENUM, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_MATRIX_5X4,
                D2D1_SATURATION_PROP_SATURATION, D2D1CreateDevice, ID2D1DeviceContext, ID2D1Image,
            },
            Direct3D::{
                D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
                D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::D3DCompile,
                ID3DBlob,
            },
            Direct3D11::*,
            Dwm::DwmExtendFrameIntoClientArea,
            Dxgi::{Common::*, *},
            Gdi::{
                CreateRoundRectRgn, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
                MonitorFromPoint, SetWindowRgn,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::MARGINS,
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{
                MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture, VK_ESCAPE,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{Interface, Result, s, w},
};

mod demo_ui;

// Lens size in logical pixels at 96 DPI.
// liquid-glass-example: w-72 content + 32px horizontal padding on each side;
// 188px of User Info content + 24px vertical padding on each side.
const LENS_W: i32 = 352;
const LENS_H: i32 = 236;

// Outline anti-aliasing. Increase both slightly for a softer edge.
const AA_DERIVATIVE_SCALE: f32 = 0.75;
const AA_MIN_HALF_WIDTH: f32 = 0.75;
const STANDARD_MAP_RB: &[u8; 256 * 256 * 2] = include_bytes!("assets/standard-map-rb.bin");

/// Visual presets exposed by the executable and examples.
#[derive(Clone, Copy, Debug, Default)]
pub enum Preset {
    #[default]
    FrostedLiquid,
}

#[derive(Clone, Copy)]
struct EffectStyle {
    displacement_scale: f32,
    blur_amount: f32,
    saturation: f32,
    chromatic_offset: f32,
    elasticity: f32,
    corner_radius: f32,
    over_light: bool,
    refraction_mode: f32,
}

impl Preset {
    fn style(self) -> EffectStyle {
        match self {
            Self::FrostedLiquid => EffectStyle {
                displacement_scale: 100.0,
                blur_amount: 0.5,
                saturation: 1.40,
                chromatic_offset: 2.0,
                elasticity: 0.0,
                corner_radius: 32.0,
                over_light: false,
                refraction_mode: 0.0,
            },
        }
    }
}

static mut DRAGGING: bool = false;
static mut DRAG_CURSOR: POINT = POINT { x: 0, y: 0 };
static mut DRAG_LENS: POINT = POINT { x: 0, y: 0 };
static mut LENS_POSITION: POINT = POINT { x: 420, y: 80 };
static mut LENS_SIZE: POINT = POINT {
    x: LENS_W,
    y: LENS_H,
};
static mut LENS_DPI: u32 = 96;
static mut LIVE_STYLE: Option<EffectStyle> = None;
static mut CONTENT_HWND: Option<HWND> = None;
static mut PANEL_HWND: Option<HWND> = None;

unsafe fn position_demo_windows(input: HWND) {
    let collapsed = demo_ui::collapsed_height();
    let _ = SetWindowPos(
        input,
        None,
        LENS_POSITION.x,
        LENS_POSITION.y,
        0,
        0,
        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    if let Some(content) = CONTENT_HWND {
        let _ = SetWindowPos(
            content,
            Some(HWND_TOPMOST),
            LENS_POSITION.x,
            LENS_POSITION.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    if let Some(panel) = PANEL_HWND {
        let _ = SetWindowPos(
            panel,
            Some(HWND_TOPMOST),
            LENS_POSITION.x,
            LENS_POSITION.y + LENS_SIZE.y - collapsed,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

unsafe fn keep_demo_in_work_area(input: HWND) {
    let monitor = MonitorFromPoint(LENS_POSITION, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        let margin = scale_for_dpi(8, LENS_DPI);
        let total_height = LENS_SIZE.y + demo_ui::glass_extension();
        let min_y = info.rcWork.top + margin;
        let max_y = (info.rcWork.bottom - margin - total_height).max(min_y);
        let fitted_y = LENS_POSITION.y.clamp(min_y, max_y);
        if fitted_y != LENS_POSITION.y {
            LENS_POSITION.y = fitted_y;
            position_demo_windows(input);
        }
    }
}

fn update_live_style(style: EffectStyle) {
    unsafe { LIVE_STYLE = Some(style) };
}

fn scale_for_dpi(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

fn scale_effect_for_dpi(value: f32, dpi: u32) -> f32 {
    value * dpi as f32 / 96.0
}

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
    effect_settings: [f32; 4],
    interaction_settings: [f32; 4],
    mouse_offset: [f32; 2],
    aa_settings: [f32; 2],
}

const SHADER: &str = r#"
cbuffer C : register(b0) {
 float2 outputOrigin, outputSize, windowOrigin, windowSize, lensSize, padding;
 float4 shadowSettings, effectSettings, interactionSettings;
 float2 mouseOffset, aaSettings;
};
Texture2D desktop : register(t0);
Texture2D displacementMap : register(t1);
SamplerState samp : register(s0);
struct V { float4 pos:SV_POSITION; float2 uv:TEXCOORD0; };
V VSMain(uint id:SV_VertexID) { V o; float2 p=float2((id<<1)&2,id&2); o.uv=p; o.pos=float4(p*float2(2,-2)+float2(-1,1),0,1); return o; }
float sdf(float2 p,float2 halfSize,float radius) { float2 q=abs(p)-halfSize+radius; return min(max(q.x,q.y),0)+length(max(q,0))-radius; }
float gradientAlpha(float position,float stopA,float stopB,float alphaA,float alphaB) {
 if(position<=stopA) return lerp(0,alphaA,saturate(position/max(stopA,.0001)));
 if(position<=stopB) return lerp(alphaA,alphaB,saturate((position-stopA)/max(stopB-stopA,.0001)));
 return lerp(alphaB,0,saturate((position-stopB)/max(1-stopB,.0001)));
}
float3 overlayWhite(float3 backdrop) {
 return float3(
  backdrop.r<=.5 ? backdrop.r*2 : 1,
  backdrop.g<=.5 ? backdrop.g*2 : 1,
  backdrop.b<=.5 ? backdrop.b*2 : 1);
}
float3 overlayBlack(float3 backdrop) {
 return float3(
  backdrop.r<=.5 ? 0 : backdrop.r*2-1,
  backdrop.g<=.5 ? 0 : backdrop.g*2-1,
  backdrop.b<=.5 ? 0 : backdrop.b*2-1);
}
float3 sampleGlass(float2 uv) {
 return desktop.Sample(samp,uv).rgb;
}
float3 mapDisplaced(float2 sampleLocal,float2 mapSideSize,float2 texel,float scale,float aberration) {
 float mapSide=max(mapSideSize.x,mapSideSize.y);
 float2 mapUv=(sampleLocal-mapSideSize*.5)/mapSide+.5;
 float2 mapOffset=displacementMap.SampleLevel(samp,mapUv,0).rg-.5;
 float2 screenUv=(windowOrigin+padding+sampleLocal-outputOrigin)/outputSize;
 float3 result;
 result.r=desktop.Sample(samp,screenUv+mapOffset*(-scale)*texel).r;
 result.g=desktop.Sample(samp,screenUv+mapOffset*(-scale*(1+aberration*.05))*texel).g;
 result.b=desktop.Sample(samp,screenUv+mapOffset*(-scale*(1+aberration*.10))*texel).b;
 return result;
}
float4 PSMain(V i):SV_TARGET {
 float2 px=i.uv*windowSize, local=px-padding, p=local-lensSize*.5;
 float elasticity=interactionSettings.y;
 float mouseLength=length(mouseOffset);
 float2 mouseDirection=mouseOffset/max(mouseLength,.001);
 float stretchIntensity=min(mouseLength/100,1)*elasticity;
 float2 shapeScale=float2(
  1+abs(mouseDirection.x)*stretchIntensity*.3-abs(mouseDirection.y)*stretchIntensity*.15,
  1+abs(mouseDirection.y)*stretchIntensity*.3-abs(mouseDirection.x)*stretchIntensity*.15);
 float2 shapeP=p/shapeScale;
 float radius=min(lensSize.y*.5,interactionSettings.z);
 float d=sdf(shapeP,lensSize*.5,radius); float sd=sdf(shapeP-float2(0,shadowSettings.x),lensSize*.5,radius);
 float shadowSigma=max(shadowSettings.y*.5,.001);
 float shadow=exp(-.5*pow(max(sd,0)/shadowSigma,2))*shadowSettings.z;
 float2 uv=local/lensSize;
 float mode=interactionSettings.w;
 float2 baseScreenUv=(windowOrigin+padding+local-outputOrigin)/outputSize, texel=1/outputSize;
 float2 normal=float2(
  sdf(shapeP+float2(1,0),lensSize*.5,radius)-sdf(shapeP-float2(1,0),lensSize*.5,radius),
  sdf(shapeP+float2(0,1),lensSize*.5,radius)-sdf(shapeP-float2(0,1),lensSize*.5,radius));
 normal=normalize(normal+float2(.0001,.0001));
 float edge=saturate(1-smoothstep(0,effectSettings.x,max(-d,0)));
 float profile;
 if(mode<.5) profile=edge*edge;
 else if(mode<1.5) profile=pow(edge,1.35);
 else if(mode<2.5) profile=pow(edge,.68);
 else profile=smoothstep(0,1,edge)*smoothstep(0,1,edge);
 float aberration=effectSettings.y;
 float3 col;
 if(mode<.5) {
  // The standard source is an opaque JPEG, so React's EDGE_MASK alpha is 1
  // across the complete filter region. Apply the map without an extra rim fade.
  float3 mapped=mapDisplaced(local,lensSize,texel,abs(interactionSettings.x),aberration);
  // xMidYMid slice crops strong side vectors when the unified control panel
  // makes the surface tall. One 0.471 fallback matches the JPEG rim's measured
  // average vector magnitude; do not attenuate it a second time while blending.
  float2 rimDisplacement=-normal*edge*abs(interactionSettings.x)*.471*texel;
  float3 rim;
  rim.r=desktop.Sample(samp,baseScreenUv+rimDisplacement).r;
  rim.g=desktop.Sample(samp,baseScreenUv+rimDisplacement*(1+aberration*.05)).g;
  rim.b=desktop.Sample(samp,baseScreenUv+rimDisplacement*(1+aberration*.10)).b;
  col=lerp(mapped,rim,edge);
 } else {
  float displacementScale=abs(interactionSettings.x)*(interactionSettings.x<0 ? .5 : 1)*.18;
  float2 displacement=-normal*profile*displacementScale*texel;
  col.r=sampleGlass(baseScreenUv+displacement).r;
  col.g=sampleGlass(baseScreenUv+displacement*(1+aberration*.05)).g;
  col.b=sampleGlass(baseScreenUv+displacement*(1+aberration*.10)).b;
 }
 // liquid-glass-example avatar: 48px circle at (32px, 68px), bg-black/10.
 float referenceScale=lensSize.x/352;
 float2 avatarCenter=float2(56,92)*referenceScale;
 float avatarDistance=length(local-avatarCenter)-24*referenceScale;
 float avatarMask=smoothstep(max(fwidth(avatarDistance),.5),-max(fwidth(avatarDistance),.5),avatarDistance);
 col=lerp(col,0,avatarMask*.1);
 if(interactionSettings.x<0) {
  col=overlayBlack(col*.8);
 }
 // CSS linear-gradient angles start at "to top" and rotate clockwise.
 // Convert that convention to screen-space (+Y points down), then normalize
 // against the projected rectangular bounds rather than square UV space.
 float angle=radians(135+mouseOffset.x*1.2);
 float2 gradientDirection=float2(sin(angle),-cos(angle));
 float gradientExtent=max(
  dot(abs(gradientDirection),lensSize*.5),
  .0001);
 float gradientPosition=dot(local-lensSize*.5,gradientDirection)/(gradientExtent*2)+.5;
 float stopA=clamp((33+mouseOffset.y*.3)/100,.1,.9);
 float stopB=clamp((66+mouseOffset.y*.4)/100,.1,.9);
 float innerDistance=max(-d,0);
 float borderMask=saturate(1-smoothstep(0,1.5,innerDistance));
 float halfPixelOutline=1-smoothstep(.25,.75,innerDistance);
 float insetDirection=saturate(.5-normal.y*.5);
 float insetGlow=exp(-.5*pow(innerDistance/1.5,2))*insetDirection*.25;
 float insetAlpha=saturate(halfPixelOutline*.5+insetGlow)*borderMask;
 float alpha1=saturate(gradientAlpha(
  gradientPosition,stopA,stopB,
  .12+abs(mouseOffset.x)*.008,
  .4+abs(mouseOffset.x)*.012))*borderMask*.2;
 alpha1=1-(1-alpha1)*(1-insetAlpha*.2);
 col=1-(1-col)*(1-alpha1);
 float alpha2=saturate(gradientAlpha(
  gradientPosition,stopA,stopB,
  .32+abs(mouseOffset.x)*.008,
  .6+abs(mouseOffset.x)*.012))*borderMask;
 alpha2=1-(1-alpha2)*(1-insetAlpha);
 col=lerp(col,overlayWhite(col),alpha2);
 float aa=max(fwidth(d)*aaSettings.x,aaSettings.y);
 float glassAlpha=smoothstep(aa,-aa,d);
 float alpha=glassAlpha+shadow*(1-glassAlpha);
 return float4(col*glassAlpha,alpha);
}"#;

struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    d2d_context: ID2D1DeviceContext,
    swap_chain: IDXGISwapChain,
    target: ID3D11RenderTargetView,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    constants: ID3D11Buffer,
    displacement_view: ID3D11ShaderResourceView,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
    outputs: Vec<OutputCapture>,
    width: i32,
    height: i32,
    style: EffectStyle,
    light_direction: [f32; 2],
    freeze_capture: bool,
}

struct OutputCapture {
    duplication: IDXGIOutputDuplication,
    desktop_texture: Option<ID3D11Texture2D>,
    desktop_view: Option<ID3D11ShaderResourceView>,
    blurred_texture: Option<ID3D11Texture2D>,
    blurred_view: Option<ID3D11ShaderResourceView>,
    map_texture: Option<ID3D11Texture2D>,
    effect_texture: Option<ID3D11Texture2D>,
    effect_view: Option<ID3D11ShaderResourceView>,
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
    fn has_desktop_frame(&self) -> bool {
        self.outputs
            .iter()
            .any(|output| output.desktop_view.is_some())
    }

    unsafe fn new(hwnd: HWND, width: i32, height: i32, preset: Preset) -> Result<Self> {
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
        let dxgi_device: IDXGIDevice = device.cast()?;
        let d2d_device = D2D1CreateDevice(&dxgi_device, None)?;
        let d2d_context = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
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
        let displacement_desc = D3D11_TEXTURE2D_DESC {
            Width: 256,
            Height: 256,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let displacement_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: STANDARD_MAP_RB.as_ptr().cast(),
            SysMemPitch: 256 * 2,
            ..Default::default()
        };
        let mut displacement_texture = None;
        device.CreateTexture2D(
            &displacement_desc,
            Some(&displacement_data),
            Some(&mut displacement_texture),
        )?;
        let mut displacement_view = None;
        device.CreateShaderResourceView(
            displacement_texture.as_ref().unwrap(),
            None,
            Some(&mut displacement_view),
        )?;
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
            d2d_context,
            swap_chain,
            target: target.unwrap(),
            vertex_shader: vertex_shader.unwrap(),
            pixel_shader: pixel_shader.unwrap(),
            constants: constants.unwrap(),
            displacement_view: displacement_view.unwrap(),
            sampler: sampler.unwrap(),
            rasterizer: rasterizer.unwrap(),
            outputs: Vec::new(),
            width,
            height,
            style: preset.style(),
            light_direction: [0.0, -1.0],
            freeze_capture: false,
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
                blurred_texture: None,
                blurred_view: None,
                map_texture: None,
                effect_texture: None,
                effect_view: None,
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
                    desc.MipLevels = 1;
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
                    let mut blur_desc = desc;
                    blur_desc.BindFlags =
                        (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32;
                    let mut blurred = None;
                    if device
                        .CreateTexture2D(&blur_desc, None, Some(&mut blurred))
                        .is_ok()
                    {
                        output.blurred_texture = blurred;
                        let _ = device.CreateShaderResourceView(
                            output.blurred_texture.as_ref().unwrap(),
                            None,
                            Some(&mut output.blurred_view),
                        );
                    }
                    let mut map = None;
                    if device
                        .CreateTexture2D(&blur_desc, None, Some(&mut map))
                        .is_ok()
                    {
                        output.map_texture = map;
                    }
                    let mut effect = None;
                    if device
                        .CreateTexture2D(&blur_desc, None, Some(&mut effect))
                        .is_ok()
                    {
                        output.effect_texture = effect;
                        let _ = device.CreateShaderResourceView(
                            output.effect_texture.as_ref().unwrap(),
                            None,
                            Some(&mut output.effect_view),
                        );
                    }
                }
                if let Some(texture) = &output.desktop_texture {
                    context.CopyResource(texture, &frame);
                }
            }
        }
        let _ = duplication.ReleaseFrame();
    }

    unsafe fn blur_output(
        &self,
        output: &OutputCapture,
        sigma: f32,
        saturation: f32,
        _visual_height: i32,
    ) {
        let (Some(source_texture), Some(target_texture)) = (
            output.desktop_texture.as_ref(),
            output.blurred_texture.as_ref(),
        ) else {
            return;
        };
        let mut texture_desc = D3D11_TEXTURE2D_DESC::default();
        source_texture.GetDesc(&mut texture_desc);
        let pixel_format = D2D1_PIXEL_FORMAT {
            format: texture_desc.Format,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        };
        let source_properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            ..Default::default()
        };
        let target_properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        let Ok(source_surface) = source_texture.cast::<IDXGISurface>() else {
            return;
        };
        let Ok(target_surface) = target_texture.cast::<IDXGISurface>() else {
            return;
        };
        let Ok(source_bitmap) = self
            .d2d_context
            .CreateBitmapFromDxgiSurface(&source_surface, Some(&source_properties))
        else {
            return;
        };
        let Ok(target_bitmap) = self
            .d2d_context
            .CreateBitmapFromDxgiSurface(&target_surface, Some(&target_properties))
        else {
            return;
        };
        let Ok(blur) = self.d2d_context.CreateEffect(&CLSID_D2D1GaussianBlur) else {
            return;
        };
        blur.SetInput(0, &source_bitmap, true);
        let sigma_bytes = slice::from_raw_parts((&sigma as *const f32).cast::<u8>(), 4);
        if blur
            .SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                sigma_bytes,
            )
            .is_err()
        {
            return;
        }
        let Ok(blurred_image) = blur.GetOutput() else {
            return;
        };
        let Ok(saturate_effect) = self.d2d_context.CreateEffect(&CLSID_D2D1Saturation) else {
            return;
        };
        saturate_effect.SetInput(0, &blurred_image, true);
        let saturation_bytes = slice::from_raw_parts((&saturation as *const f32).cast::<u8>(), 4);
        if saturate_effect
            .SetValue(
                D2D1_SATURATION_PROP_SATURATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                saturation_bytes,
            )
            .is_err()
        {
            return;
        }
        let Ok(saturated_image) = saturate_effect.GetOutput() else {
            return;
        };
        self.d2d_context.SetTarget(&target_bitmap);
        self.d2d_context.BeginDraw();
        self.d2d_context.DrawImage(
            &saturated_image,
            None,
            None,
            D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_COMPOSITE_MODE_SOURCE_COPY,
        );
        let _ = self.d2d_context.EndDraw(None, None);
        self.d2d_context.SetTarget(None::<&ID2D1Image>);
    }

    unsafe fn refract_output(
        &self,
        output: &OutputCapture,
        scale: f32,
        aberration: f32,
        post_blur: f32,
        visual_height: i32,
    ) {
        let (Some(source_texture), Some(map_texture), Some(target_texture)) = (
            output.blurred_texture.as_ref(),
            output.map_texture.as_ref(),
            output.effect_texture.as_ref(),
        ) else {
            return;
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        source_texture.GetDesc(&mut desc);
        let pixel_format = D2D1_PIXEL_FORMAT {
            format: desc.Format,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        };
        let source_properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            ..Default::default()
        };
        let target_properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        let (Ok(source_surface), Ok(map_surface), Ok(target_surface)) = (
            source_texture.cast::<IDXGISurface>(),
            map_texture.cast::<IDXGISurface>(),
            target_texture.cast::<IDXGISurface>(),
        ) else {
            return;
        };
        let (Ok(source), Ok(map_target), Ok(target)) = (
            self.d2d_context
                .CreateBitmapFromDxgiSurface(&source_surface, Some(&source_properties)),
            self.d2d_context
                .CreateBitmapFromDxgiSurface(&map_surface, Some(&target_properties)),
            self.d2d_context
                .CreateBitmapFromDxgiSurface(&target_surface, Some(&target_properties)),
        ) else {
            return;
        };

        // Direct2D bitmaps use BGRA memory order. The React map uses R for X and B for Y.
        let mut map_pixels = vec![0u8; 256 * 256 * 4];
        for (source, pixel) in STANDARD_MAP_RB
            .chunks_exact(2)
            .zip(map_pixels.chunks_exact_mut(4))
        {
            pixel.copy_from_slice(&[source[1], 128, source[0], 255]);
        }
        let map_properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            ..Default::default()
        };
        let Ok(map_source) = self.d2d_context.CreateBitmap(
            D2D_SIZE_U {
                width: 256,
                height: 256,
            },
            Some(map_pixels.as_ptr().cast()),
            256 * 4,
            &map_properties,
        ) else {
            return;
        };

        self.d2d_context.SetTarget(&map_target);
        self.d2d_context.BeginDraw();
        let neutral = D2D1_COLOR_F {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        };
        self.d2d_context.Clear(Some(&neutral));
        let destination = D2D_RECT_F {
            left: (LENS_POSITION.x - output.rect.left) as f32,
            top: (LENS_POSITION.y - output.rect.top) as f32,
            right: (LENS_POSITION.x + LENS_SIZE.x - output.rect.left) as f32,
            bottom: (LENS_POSITION.y + visual_height - output.rect.top) as f32,
        };
        let destination_aspect = LENS_SIZE.x as f32 / visual_height.max(1) as f32;
        let source_rect = if destination_aspect >= 1.0 {
            let height = 256.0 / destination_aspect;
            D2D_RECT_F {
                left: 0.0,
                top: (256.0 - height) * 0.5,
                right: 256.0,
                bottom: (256.0 + height) * 0.5,
            }
        } else {
            let width = 256.0 * destination_aspect;
            D2D_RECT_F {
                left: (256.0 - width) * 0.5,
                top: 0.0,
                right: (256.0 + width) * 0.5,
                bottom: 256.0,
            }
        };
        self.d2d_context.DrawBitmap(
            &map_source,
            Some(&destination),
            1.0,
            D2D1_INTERPOLATION_MODE_LINEAR,
            Some(&source_rect),
            None,
        );
        if self.d2d_context.EndDraw(None, None).is_err() {
            self.d2d_context.SetTarget(None::<&ID2D1Image>);
            return;
        }
        self.d2d_context.SetTarget(None::<&ID2D1Image>);

        let set_float = |effect: &windows::Win32::Graphics::Direct2D::ID2D1Effect,
                         property: u32,
                         value: f32| {
            let bytes =
                slice::from_raw_parts((&value as *const f32).cast::<u8>(), size_of::<f32>());
            effect.SetValue(property, D2D1_PROPERTY_TYPE_FLOAT, bytes)
        };
        let set_enum = |effect: &windows::Win32::Graphics::Direct2D::ID2D1Effect,
                        property: u32,
                        value: i32| {
            let bytes =
                slice::from_raw_parts((&value as *const i32).cast::<u8>(), size_of::<i32>());
            effect.SetValue(property, D2D1_PROPERTY_TYPE_ENUM, bytes)
        };
        let mut channels = Vec::with_capacity(3);
        let scales = [
            -scale,
            scale * (-1.0 - aberration * 0.05),
            scale * (-1.0 - aberration * 0.10),
        ];
        for (index, channel_scale) in scales.into_iter().enumerate() {
            let Ok(displace) = self.d2d_context.CreateEffect(&CLSID_D2D1DisplacementMap) else {
                return;
            };
            displace.SetInput(0, &source, true);
            displace.SetInput(1, &map_target, true);
            if set_float(
                &displace,
                D2D1_DISPLACEMENTMAP_PROP_SCALE.0 as u32,
                channel_scale,
            )
            .is_err()
                || set_enum(
                    &displace,
                    D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT.0 as u32,
                    D2D1_CHANNEL_SELECTOR_R.0,
                )
                .is_err()
                || set_enum(
                    &displace,
                    D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT.0 as u32,
                    D2D1_CHANNEL_SELECTOR_B.0,
                )
                .is_err()
            {
                return;
            }
            let Ok(displaced) = displace.GetOutput() else {
                return;
            };
            let Ok(matrix_effect) = self.d2d_context.CreateEffect(&CLSID_D2D1ColorMatrix) else {
                return;
            };
            matrix_effect.SetInput(0, &displaced, true);
            let mut values = [0.0f32; 20];
            values[index * 5] = 1.0;
            values[15] = 1.0;
            let matrix = D2D_MATRIX_5X4_F {
                Anonymous: windows::Win32::Graphics::Direct2D::Common::D2D_MATRIX_5X4_F_0 {
                    m: values,
                },
            };
            let bytes = slice::from_raw_parts(
                (&matrix as *const D2D_MATRIX_5X4_F).cast::<u8>(),
                size_of::<D2D_MATRIX_5X4_F>(),
            );
            if matrix_effect
                .SetValue(
                    D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                    D2D1_PROPERTY_TYPE_MATRIX_5X4,
                    bytes,
                )
                .is_err()
            {
                return;
            }
            let Ok(channel) = matrix_effect.GetOutput() else {
                return;
            };
            channels.push(channel);
        }
        let screen = |first: &ID2D1Image, second: &ID2D1Image| {
            let effect = self.d2d_context.CreateEffect(&CLSID_D2D1Blend)?;
            effect.SetInput(0, first, true);
            effect.SetInput(1, second, true);
            set_enum(
                &effect,
                D2D1_BLEND_PROP_MODE.0 as u32,
                D2D1_BLEND_MODE_SCREEN.0,
            )?;
            effect.GetOutput()
        };
        let Ok(gb) = screen(&channels[1], &channels[2]) else {
            return;
        };
        let Ok(rgb) = screen(&channels[0], &gb) else {
            return;
        };
        let Ok(soften) = self.d2d_context.CreateEffect(&CLSID_D2D1GaussianBlur) else {
            return;
        };
        soften.SetInput(0, &rgb, true);
        if set_float(
            &soften,
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
            post_blur,
        )
        .is_err()
        {
            return;
        }
        let Ok(final_image) = soften.GetOutput() else {
            return;
        };
        self.d2d_context.SetTarget(&target);
        self.d2d_context.BeginDraw();
        self.d2d_context.DrawImage(
            &final_image,
            None,
            None,
            D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_COMPOSITE_MODE_SOURCE_COPY,
        );
        let _ = self.d2d_context.EndDraw(None, None);
        self.d2d_context.SetTarget(None::<&ID2D1Image>);
    }

    unsafe fn render(&mut self, hwnd: HWND) {
        let style = LIVE_STYLE.unwrap_or(self.style);
        let glass_extension = demo_ui::glass_extension();
        let visual_height = LENS_SIZE.y + glass_extension;
        for output in &mut self.outputs {
            if !self.freeze_capture || output.desktop_texture.is_none() {
                Self::capture_output(&self.device, &self.context, output);
            }
        }
        let blur_sigma = scale_effect_for_dpi(
            (if style.over_light { 12.0 } else { 4.0 }) + style.blur_amount * 32.0,
            LENS_DPI,
        );
        for output in &self.outputs {
            self.blur_output(output, blur_sigma, style.saturation, visual_height);
            self.refract_output(
                output,
                scale_effect_for_dpi(style.displacement_scale, LENS_DPI),
                style.chromatic_offset,
                scale_effect_for_dpi((0.5 - style.chromatic_offset * 0.1).max(0.1), LENS_DPI),
                visual_height,
            );
        }
        let mut window = RECT::default();
        if GetWindowRect(hwnd, &mut window).is_err() {
            return;
        }
        let mut cursor = POINT::default();
        let monitor = MonitorFromPoint(LENS_POSITION, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let target_light = if GetCursorPos(&mut cursor).is_ok()
            && GetMonitorInfoW(monitor, &mut monitor_info).as_bool()
        {
            let rect = monitor_info.rcMonitor;
            let width = (rect.right - rect.left).max(1) as f32;
            let height = (rect.bottom - rect.top).max(1) as f32;
            [
                (cursor.x as f32 - (rect.left + rect.right) as f32 * 0.5) / width * 100.0,
                (cursor.y as f32 - (rect.top + rect.bottom) as f32 * 0.5) / height * 100.0,
            ]
        } else {
            [0.0, 0.0]
        };
        let light_blend = 0.14;
        self.light_direction[0] += (target_light[0] - self.light_direction[0]) * light_blend;
        self.light_direction[1] += (target_light[1] - self.light_direction[1]) * light_blend;
        let light_direction = self.light_direction;
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
            let Some(view) = output.blurred_view.clone() else {
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
                lens_size: [LENS_SIZE.x as f32, visual_height as f32],
                padding: [
                    (LENS_POSITION.x - window.left) as f32,
                    (LENS_POSITION.y - window.top) as f32,
                ],
                shadow_settings: [
                    scale_effect_for_dpi(if style.over_light { 16.0 } else { 12.0 }, LENS_DPI),
                    scale_effect_for_dpi(if style.over_light { 70.0 } else { 40.0 }, LENS_DPI),
                    if style.over_light { 0.75 } else { 0.25 },
                    0.0,
                ],
                effect_settings: [
                    scale_effect_for_dpi(30.0, LENS_DPI),
                    style.chromatic_offset,
                    scale_effect_for_dpi((0.5 - style.chromatic_offset * 0.1).max(0.1), LENS_DPI),
                    style.saturation,
                ],
                interaction_settings: [
                    scale_effect_for_dpi(
                        if style.over_light {
                            -style.displacement_scale * 0.5
                        } else {
                            style.displacement_scale
                        },
                        LENS_DPI,
                    ),
                    style.elasticity,
                    scale_effect_for_dpi(style.corner_radius, LENS_DPI),
                    style.refraction_mode,
                ],
                mouse_offset: light_direction,
                aa_settings: [
                    AA_DERIVATIVE_SCALE,
                    scale_effect_for_dpi(AA_MIN_HALF_WIDTH, LENS_DPI),
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
            let effect_margin = (LENS_SIZE.x as f32 * 0.35).ceil() as i32;
            self.context.RSSetScissorRects(Some(&[RECT {
                left: (LENS_POSITION.x - window.left - effect_margin)
                    .max(output.rect.left - window.left)
                    .max(0),
                top: (LENS_POSITION.y - window.top - effect_margin)
                    .max(output.rect.top - window.top)
                    .max(0),
                right: (LENS_POSITION.x + LENS_SIZE.x - window.left + effect_margin)
                    .min(output.rect.right - window.left)
                    .min(self.width),
                bottom: (LENS_POSITION.y + visual_height - window.top + effect_margin)
                    .min(output.rect.bottom - window.top)
                    .min(self.height),
            }]));
            self.context
                .PSSetShaderResources(0, Some(&[Some(view), Some(self.displacement_view.clone())]));
            self.context.Draw(3, 0);
            self.context.PSSetShaderResources(0, Some(&[None, None]));
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
                let x = (DRAG_LENS.x + cursor.x - DRAG_CURSOR.x).clamp(
                    virtual_left,
                    (virtual_right - LENS_SIZE.x).max(virtual_left),
                );
                let y = (DRAG_LENS.y + cursor.y - DRAG_CURSOR.y)
                    .clamp(virtual_top, (virtual_bottom - LENS_SIZE.y).max(virtual_top));
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
                    if let Some(content) = CONTENT_HWND {
                        let _ = SetWindowPos(
                            content,
                            Some(HWND_TOPMOST),
                            x,
                            y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                    if let Some(panel) = PANEL_HWND {
                        let _ = SetWindowPos(
                            panel,
                            Some(HWND_TOPMOST),
                            x,
                            y + LENS_SIZE.y - demo_ui::collapsed_height(),
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            DRAGGING = false;
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xffff) as u32;
            if new_dpi != 0 && new_dpi != LENS_DPI {
                let old_size = LENS_SIZE;
                LENS_DPI = new_dpi;
                LENS_SIZE = POINT {
                    x: scale_for_dpi(LENS_W, new_dpi),
                    y: scale_for_dpi(LENS_H, new_dpi),
                };
                LENS_POSITION.x += (old_size.x - LENS_SIZE.x) / 2;
                LENS_POSITION.y += (old_size.y - LENS_SIZE.y) / 2;
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    LENS_POSITION.x,
                    LENS_POSITION.y,
                    LENS_SIZE.x,
                    LENS_SIZE.y,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Some(content) = CONTENT_HWND {
                    let _ = SetWindowPos(
                        content,
                        Some(HWND_TOPMOST),
                        LENS_POSITION.x,
                        LENS_POSITION.y,
                        LENS_SIZE.x,
                        LENS_SIZE.y,
                        SWP_NOACTIVATE,
                    );
                }
                if let Some(panel) = PANEL_HWND {
                    let _ = SetWindowPos(
                        panel,
                        Some(HWND_TOPMOST),
                        LENS_POSITION.x,
                        LENS_POSITION.y + LENS_SIZE.y - demo_ui::collapsed_height(),
                        LENS_SIZE.x,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                let region = CreateRoundRectRgn(
                    0,
                    0,
                    LENS_SIZE.x + 1,
                    LENS_SIZE.y + 1,
                    LENS_SIZE.y,
                    LENS_SIZE.y,
                );
                let _ = SetWindowRgn(hwnd, Some(region), true);
                if DRAGGING {
                    DRAG_LENS = LENS_POSITION;
                    let _ = GetCursorPos(&mut DRAG_CURSOR);
                }
            }
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

pub fn run(preset: Preset) -> Result<()> {
    unsafe {
        LIVE_STYLE = Some(preset.style());
        let screenshot_mode = std::env::var_os("LIQUID_GLASS_SCREENSHOT").is_some();
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
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
            WS_POPUP,
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
        if !screenshot_mode {
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
        }
        RegisterHotKey(Some(hwnd), 1, MOD_NOREPEAT, VK_ESCAPE.0 as u32)?;
        let input_hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            input_name,
            w!("Liquid Glass Input"),
            WS_POPUP,
            LENS_POSITION.x,
            LENS_POSITION.y,
            LENS_W,
            LENS_H,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?;
        LENS_DPI = GetDpiForWindow(input_hwnd);
        LENS_SIZE = POINT {
            x: scale_for_dpi(LENS_W, LENS_DPI),
            y: scale_for_dpi(LENS_H, LENS_DPI),
        };
        let _ = SetWindowPos(
            input_hwnd,
            None,
            LENS_POSITION.x,
            LENS_POSITION.y,
            LENS_SIZE.x,
            LENS_SIZE.y,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        SetLayeredWindowAttributes(
            input_hwnd,
            windows::Win32::Foundation::COLORREF(0),
            1,
            LWA_ALPHA,
        )?;
        let input_region = CreateRoundRectRgn(
            0,
            0,
            LENS_SIZE.x + 1,
            LENS_SIZE.y + 1,
            LENS_SIZE.y,
            LENS_SIZE.y,
        );
        SetWindowRgn(input_hwnd, Some(input_region), true);
        if !screenshot_mode {
            SetWindowDisplayAffinity(input_hwnd, WDA_EXCLUDEFROMCAPTURE)?;
        }
        let mut renderer = Renderer::new(hwnd, screen_width, screen_height, preset)?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = ShowWindow(input_hwnd, SW_SHOW);
        if screenshot_mode {
            for _ in 0..20 {
                renderer.render(hwnd);
                if renderer.has_desktop_frame() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            renderer.freeze_capture = true;
        }
        if matches!(preset, Preset::FrostedLiquid) {
            let (content, panel, panel_input) = demo_ui::create_demo_windows(
                HINSTANCE(module.0),
                LENS_POSITION,
                LENS_SIZE,
                preset.style(),
            )?;
            if !screenshot_mode {
                SetWindowDisplayAffinity(content, WDA_EXCLUDEFROMCAPTURE)?;
                SetWindowDisplayAffinity(panel, WDA_EXCLUDEFROMCAPTURE)?;
                SetWindowDisplayAffinity(panel_input, WDA_EXCLUDEFROMCAPTURE)?;
            }
            CONTENT_HWND = Some(content);
            PANEL_HWND = Some(panel);
        }
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
            if matches!(preset, Preset::FrostedLiquid) {
                demo_ui::tick();
                keep_demo_in_work_area(input_hwnd);
            }
        }
    }
}
