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
- 基于 SDF 像素导数的轮廓抗锯齿
- 捕获排除，透镜窗口不会递归出现在自己的画面中
- 所有常用视觉参数集中在 Rust 源文件顶部

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

该示例在相同的桌面折射基础上增加多采样背景模糊、140% 饱和度、更明显的 RGB
色散、柔和阴影，以及随鼠标位置改变方向的边缘高光。示例使用
`Preset::FrostedLiquid`，默认程序仍使用 `Preset::Standard`，两者可以独立调整。

示例保留了 `User Info` 内容。内容由独立透明窗口绘制，因此保持清晰，不会随背景一起
折射或模糊。玻璃底部的 `Settings` 默认折叠，点击后以阻尼动画展开自绘参数面板；再次
点击即可收起。

面板覆盖参考组件的全部视觉参数：

- `Refraction mode`：`Standard`、`Polar`、`Prominent`、`Shader`
- `Displacement scale`：折射位移强度
- `Blur amount`：磨砂背景的采样半径
- `Saturation`：玻璃区域的颜色饱和度
- `Chromatic aberration`：边缘 RGB 色散距离
- `Elasticity`：玻璃朝鼠标方向产生的弹性形变
- `Corner radius`：轮廓圆角，最大值为完整胶囊形状
- `Over light`：在明亮背景上压暗玻璃内容

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

- 在透镜区域按住鼠标左键并拖动，可以移动透镜。
- 按 `Esc` 退出程序。
- 透镜可以移动到主显示器左侧或上方的屏幕。

## 调整效果

所有常用参数都位于 `src/main.rs` 顶部。修改后重新运行 `cargo run` 或重新构建即可。

### 尺寸

| 参数 | 说明 |
| --- | --- |
| `LENS_W` | 透镜宽度，单位为物理像素 |
| `LENS_H` | 透镜高度，单位为物理像素，同时决定胶囊圆角半径 |

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
|-- Cargo.toml       # 包信息、Windows API 功能依赖
|-- Cargo.lock       # 锁定的依赖版本
`-- src/
    `-- main.rs      # Win32 窗口、桌面捕获、D3D11 渲染和 HLSL 着色器
```

## 实现概要

程序创建两个顶层窗口：一个覆盖虚拟桌面的透明渲染窗口，以及一个与透镜位置同步的
输入窗口。DXGI Desktop Duplication 捕获透镜中心所在的显示器，D3D11 像素着色器
使用有符号距离场计算胶囊轮廓，并完成折射和透明度合成。

多显示器坐标统一使用 Windows 虚拟桌面坐标，因此位于主屏左侧或上方的显示器所产生
的负坐标也能正确处理。

## 检查

```powershell
cargo fmt -- --check
cargo check
cargo test
```

当前项目没有独立的自动化测试用例，`cargo test` 主要用于确认测试配置下可以正常编译。
