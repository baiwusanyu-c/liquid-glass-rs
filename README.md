# Liquid Glass

一个使用 Rust、Win32、Direct3D 11 和 DXGI Desktop Duplication 实现的 Windows
桌面液态玻璃透镜。程序实时捕获透镜所在的显示器，通过像素着色器生成折射、色散、
边缘光、阴影和抗锯齿效果。

## 功能

- 实时桌面折射和液态玻璃效果
- 可拖动的胶囊形透镜
- 支持多显示器和虚拟桌面负坐标
- 透镜跨屏后自动切换桌面捕获源
- Per-Monitor V2 DPI 感知
- `frosted_liquid` 示例支持 HDR/scRGB 桌面捕获的色调映射
- 基于 SDF 像素导数的轮廓抗锯齿
- 捕获排除，透镜窗口不会递归出现在自己的画面中
- `frosted_liquid` 的常用视觉参数可通过常驻控制器实时调整

## 环境要求

- Windows 10 或 Windows 11
- 支持 Direct3D 11 和 DXGI Desktop Duplication 的显卡及驱动
- Rust 工具链，推荐使用 stable-x86_64-pc-windows-msvc
- Visual Studio Build Tools，并安装 Desktop development with C++ 组件

安装 Rust 后，可以确认当前工具链：

```powershell
rustup default stable-x86_64-pc-windows-msvc
rustc --version
cargo --version
```

## 运行

开发模式：

```powershell
cargo run
```

磨砂液态玻璃示例：

```powershell
cargo run --example frosted_liquid
```

该示例使用 DXGI Desktop Duplication 捕获桌面，通过 Direct2D 完成裁剪、镜像边缘扩展、
Gaussian Blur 和饱和度处理，再由 D3D11 像素着色器完成位移折射、RGB 色散、边缘高光、
阴影和弹性形变。示例窗口固定为 96 DPI 下的 `352 x 236` 逻辑像素，
只显示始终展开的参数控制器；顶部空白区域用于拖动窗口。

控制器参数会实时作用于渲染：

- `Refraction mode`：`Standard`、`Polar`、`Prominent`、`Shader`
- `Displacement scale`：折射位移强度
- `Blur amount`：Direct2D 高斯背景模糊强度
- `Saturation`：玻璃区域的颜色饱和度
- `Chromatic aberration`：边缘 RGB 色散距离
- `Elasticity`：玻璃朝鼠标方向产生的弹性形变
- `Corner radius`：轮廓圆角，最大值为完整胶囊形状
- `Over light`：在明亮背景上压暗玻璃内容

默认值为：Displacement `100`、Blur `0.5`、Saturation `140%`、Chromatic
Aberration `2`、Elasticity `0.00`、Corner Radius `32px`。

### 折射模式

- `Standard`、`Polar`、`Prominent` 使用 React 实现中原始 Base64 位移图的逐字节解码结果。
- 三张位移图以二进制资源编译进可执行文件，启动时通过 Windows Imaging Component 解码一次并上传 GPU。
- `Shader` 不使用静态位移图，位移场由 HLSL 按 React Canvas shader 的 SDF、归一化、2px 边缘衰减和 8-bit 量化规则生成。
- 四种模式均使用 R 通道作为 X 位移、B 通道作为 Y 位移，并分别计算 RGB 三路色差采样。
- backdrop blur 输入和折射采样在卡片边界使用镜像延展。该行为通过 React/Chromium 坐标编码与颜色坡度探针验证，可防止卡片外内容混入边缘。

构建发布版本：

```powershell
cargo build --release
```

生成的程序位于：

```text
target\release\liquid-glass.exe
```

这是 Windows GUI 程序，启动后不会显示控制台窗口。

## 操作

- 默认程序可在透镜区域按住鼠标左键拖动。
- `frosted_liquid` 示例通过控制器顶部的空白区域拖动；控件区域用于调整参数。
- 按 `Esc` 退出程序。
- 透镜可以移动到主显示器左侧或上方的屏幕。

## 调整效果

默认程序的常用参数位于 `src/main.rs` 顶部。`frosted_liquid` 示例的初始值位于
`examples/frosted_liquid/app.rs`，运行时也可以通过控制器调整。

### 尺寸

| 参数 | 说明 |
| --- | --- |
| `LENS_W` | 透镜宽度，单位为 96 DPI 下的逻辑像素 |
| `LENS_H` | 透镜高度，单位为 96 DPI 下的逻辑像素，同时决定默认胶囊圆角半径 |

### 阴影

| 参数 | 增大后的效果 |
| --- | --- |
| `SHADOW_OFFSET_Y` | 阴影向下移动 |
| `SHADOW_SOFTNESS` | 阴影扩散得更宽、更柔和 |
| `SHADOW_OPACITY` | 阴影更深 |

### 折射

| 参数 | 说明 |
| --- | --- |
| `REFRACTION_CORE_X` | 中心低形变区域的横向范围 |
| `REFRACTION_CORE_Y` | 中心低形变区域的纵向范围 |
| `REFRACTION_RADIUS` | 折射映射形状的圆角程度 |
| `REFRACTION_TRANSITION` | 折射从中心到边缘的过渡范围 |
| `REFRACTION_BIAS` | 增大后边缘折射更明显 |

### 边缘和颜色

| 参数 | 增大后的效果 |
| --- | --- |
| `EDGE_EFFECT_WIDTH` | 高光、暗边和色散向内部延伸得更宽 |
| `CHROMATIC_OFFSET` | 红蓝通道分离更明显 |
| `COLOR_CONTRAST` | 透镜内画面对比度更高 |
| `COLOR_GAIN` | 整体颜色更亮 |
| `COLOR_LIFT` | 黑位和暗部被抬高 |

### 光照和抗锯齿

| 参数 | 说明 |
| --- | --- |
| `TOP_LIGHT_POSITION` | 顶部高光的纵向作用位置 |
| `TOP_LIGHT_STRENGTH` | 顶部高光强度 |
| `LOWER_SHADOW_POSITION` | 底部暗边开始作用的位置 |
| `LOWER_SHADOW_STRENGTH` | 底部暗边强度 |
| `AA_DERIVATIVE_SCALE` | 根据像素导数计算的抗锯齿宽度系数 |
| `AA_MIN_HALF_WIDTH` | 最小抗锯齿半宽；增大后轮廓更柔和 |

如果高分辨率屏幕上的边缘仍然偏硬，可以小幅同时提高
`AA_DERIVATIVE_SCALE` 和 `AA_MIN_HALF_WIDTH`。数值过大会让轮廓变得模糊。

## 项目结构

```text
liquid-glass/
|-- Cargo.toml                       # 包信息、Windows API 功能依赖
|-- Cargo.lock                       # 锁定的依赖版本
|-- src/
|   `-- main.rs                      # 默认液态玻璃程序
`-- examples/
    |-- frosted_liquid.rs            # 示例入口
    `-- frosted_liquid/
        |-- app.rs                   # 捕获、D2D/D3D11 渲染和交互
        |-- demo_ui.rs               # 自绘参数控制器
        `-- embedded/                # 编译进可执行文件的 React 原始位移图
            |-- standard.jpg
            |-- polar.jpg
            `-- prominent.png
```

## 实现概要

默认程序创建两个顶层窗口：一个覆盖虚拟桌面的透明渲染窗口，以及一个与透镜位置同步的
输入窗口。程序为显卡连接的各个输出建立 DXGI Desktop Duplication，在透镜跨越屏幕边界
时分别绘制相交区域。D3D11 像素着色器使用有符号距离场计算轮廓，并完成折射和透明度合成。

`frosted_liquid` 另外创建控制器的可视窗口和透明点击窗口。两者在拖动和
`WM_DPICHANGED` 时同步位置与尺寸。Direct2D 先将捕获内容裁剪到卡片区域，通过 MIRROR
边界扩展后执行 Gaussian Blur 和饱和度处理；D3D11 像素着色器随后读取静态或程序化位移场，
对越界坐标执行相同的 MIRROR 映射，并完成 RGB 色差、轮廓和鼠标高光。
静态位移图由 WIC 从可执行文件内嵌字节解码，不访问运行时外部资源。

多显示器坐标统一使用 Windows 虚拟桌面坐标，因此位于主屏左侧或上方的显示器所产生
的负坐标也能正确处理。

## 检查

```powershell
cargo fmt --all -- --check
cargo check --all-targets
cargo test
```

当前项目没有独立的自动化测试用例，`cargo test` 主要用于确认测试配置下可以正常编译。
