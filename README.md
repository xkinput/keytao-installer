# 键道

键道输入方案与配套工具，基于 [Tauri](https://tauri.app/) 构建，支持桌面端与 Android。

## 功能

- 自动从 GitHub 获取最新键道版本并下载
- 智能合并 `default.custom.yaml`，保留用户已有的非键道方案
- 安装进度实时展示
- 自动检测 Rime 配置目录（也可手动选择）
- 安装完成后提示重新部署 Rime

## 支持平台

| 平台 | Rime 前端 | 配置目录 |
|------|-----------|----------|
| macOS | 鼠须管（Squirrel） | `~/Library/Rime` |
| Windows | 小狼毫（Weasel） | `%APPDATA%\Rime` |
| Linux | `keytao-ime`（Wayland input-method-v2 + XIM + IBus 兼容） | `~/.local/share/rime` |
| Android | 同文输入法（Trime） | `/sdcard/rime`（SAF） |
| iOS | iRime | 需手动导入，仅提供下载链接 |

## 下载

前往 [Releases](https://github.com/xkinput/keytao-app/releases) 下载对应平台的安装包。

---

## Linux 安装

### AppImage / deb

从 [Releases](https://github.com/xkinput/keytao-app/releases) 下载后直接运行（AppImage 无需安装）：

```bash
chmod +x keytao-app_*.AppImage
./keytao-app_*.AppImage
```

### NixOS / nix-darwin（推荐）

本项目提供 Nix flake，可以用 Home Manager 或 NixOS module 直接集成：

**1. 在 `flake.nix` 中添加 input**

```nix
inputs.keytao-app = {
  url = "github:xkinput/keytao-app";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

**2. 使用 Home Manager 一句安装**

```nix
imports = [ inputs.keytao-app.homeManagerModules.default ];
```

模块会安装 GUI + `keytao-ime` daemon，并在支持 `programs.niri` 的配置中自动加入启动项和输入法环境变量。

如果只想手动安装包，也可以直接引用默认包：

```nix
# home.packages（或 environment.systemPackages）
inputs.keytao-app.packages.${pkgs.stdenv.hostPlatform.system}.default
```

---

## Linux IME 架构

Linux 下只有一个协议入口：`keytao-ime`。GUI 应用只负责下载、合并和部署 Rime 配置，并在启动时确保 `keytao-ime` 已运行。

```
keytao-app（Tauri GUI）
  └── 部署 Rime 资源并启动 keytao-ime

keytao-ime（Linux IME daemon）
  ├── Wayland frontend ──→ zwp_input_method_v2 ──→ 原生 Wayland 应用
  ├── X11 frontend ──────→ XIM ──────────────────→ X11 / XWayland 应用
  └── IBus frontend ─────→ org.freedesktop.IBus ─→ GTK / Chromium / CEF 兼容路径
                           └─ preedit / lookup table / commit signals
```

`keytao-ime` 会为每个输入上下文创建独立 librime session，避免多个应用或窗口共享同一个 composition 状态。

### Wayland（原生应用）

启动 `keytao-app` 或直接启动 `keytao-ime`。GTK 应用会通过 `text-input-v3` 协议自动连接；Electron 应用需要设置以下环境变量：

```
NIXOS_OZONE_WL=1
ELECTRON_OZONE_PLATFORM_HINT=wayland
```

### XWayland（XIM）

针对使用 X11/XCB 的应用（如微信、旧版 Qt 应用），需要在应用环境中设置 `XMODIFIERS`：

```bash
XMODIFIERS=@im=keytao <your-app>
```

#### niri（Wayland compositor）配置示例

```nix
programs.niri.settings = {
  spawn-at-startup = [
    { command = [ "keytao-app" ]; }
  ];

  environment = {
    # Electron / Chromium → native Wayland
    "NIXOS_OZONE_WL" = "1";
    "ELECTRON_OZONE_PLATFORM_HINT" = "wayland";
    # GTK: prefer Wayland, fall back to X11
    "GDK_BACKEND" = "wayland,x11";
    # Qt: prefer Wayland, fall back to xcb (XWayland)
    "QT_QPA_PLATFORM" = "wayland;xcb";
    # XWayland display — niri usually assigns :0.
    "DISPLAY" = ":0";
    "XMODIFIERS" = "@im=keytao";
    "GTK_IM_MODULE" = "xim";
    "QT_IM_MODULE" = "xim";
  };
};
```

#### 微信（wechat-uos）启动脚本示例

```bash
export XMODIFIERS="@im=keytao"
exec wechat
```

微信是 Qt 应用，在 Wayland 会话中通过 XWayland（xcb）运行，不支持 `zwp_input_method_v2`，只能通过 XIM 输入中文。

---

## 开发

推荐使用 `direnv` 自动加载 flake 开发环境：

```bash
direnv allow
```

进入仓库目录后，`direnv` 会自动提供 Tauri / Android / Linux 打包所需环境变量。

```bash
pnpm install
pnpm tauri dev
```

构建：

```bash
pnpm tauri build
```

Linux 下如果要让 Tauri 包内嵌 `keytao-ime` sidecar，需要先构建 daemon 并注入 Linux-only Tauri 配置：

```bash
cargo build -p keytao-linux-ime --release
KEYTAO_IME_PATH="$PWD/target/release/keytao-ime" \
TAURI_CONFIG='{"bundle":{"externalBin":["binaries/keytao-ime"]}}' \
pnpm tauri build --bundles deb
```

Linux 打包（deb + tar.gz）：

```bash
pnpm build:linux
```

Android 构建需要配置 Android SDK 与 NDK，参考 [Tauri 移动端文档](https://tauri.app/start/prerequisites/#android)。

## 许可证

MIT
