# 键道安装器

键道输入方案的一键安装工具，基于 [Tauri](https://tauri.app/) 构建，支持桌面端与 Android。

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
| Linux | 内置 IME（zwp_input_method_v2 + XIM） | `~/.local/share/rime` |
| Android | 同文输入法（Trime） | `/sdcard/rime`（SAF） |
| iOS | iRime | 需手动导入，仅提供下载链接 |

## 下载

前往 [Releases](https://github.com/xkinput/keytao-installer/releases) 下载对应平台的安装包。

---

## Linux 安装

### AppImage / deb

从 [Releases](https://github.com/xkinput/keytao-installer/releases) 下载后直接运行（AppImage 无需安装）：

```bash
chmod +x keytao-installer_*.AppImage
./keytao-installer_*.AppImage
```

### NixOS / nix-darwin（推荐）

本项目提供 Nix flake，可以用 Home Manager 或 NixOS module 直接集成：

**1. 在 `flake.nix` 中添加 input**

```nix
inputs.keytao-installer = {
  url = "github:xkinput/keytao-installer";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

**2. 安装两个包**

```nix
# home.packages（或 environment.systemPackages）
inputs.keytao-installer.packages.${pkgs.stdenv.hostPlatform.system}.default      # GUI 安装器 + Wayland IME
inputs.keytao-installer.packages.${pkgs.stdenv.hostPlatform.system}.keytao-linux-ime  # 独立 XIM 服务器（XWayland 应用需要）
```

---

## Linux IME 架构

Linux 下有两个互补的 IME 进程，分别服务不同的应用类型：

```
keytao-installer（Tauri GUI）
  └── 内嵌 Wayland 后端 ──→ zwp_input_method_v2 ──→ 原生 Wayland 应用（GTK / Electron / …）

keytao-ime（独立守护进程）
  └── X11/XIM 服务器 ──────→ XIM 协议 ─────────→ XWayland 应用（微信 / 旧版 Qt / …）
```

两者不能合并为一个进程，因为 Wayland compositor 每个 seat 只允许**一个进程**注册 `zwp_input_method_v2`；`keytao-installer` 已占用该位置，`keytao-ime` 必须以 X11-only 模式运行（不设置 `WAYLAND_DISPLAY`）。

### Wayland（原生应用）

启动 `keytao-installer`，无需额外配置。GTK 应用会通过 `text-input-v3` 协议自动连接；Electron 应用需要设置以下环境变量：

```
NIXOS_OZONE_WL=1
ELECTRON_OZONE_PLATFORM_HINT=wayland
```

### XWayland（XIM）

针对使用 X11/XCB 的应用（如微信、旧版 Qt 应用），需要启动 `keytao-ime` 作为 XIM 服务器，并在应用环境中设置 `XMODIFIERS`：

```bash
XMODIFIERS=@im=keytao <your-app>
```

`keytao-ime` 必须在**不设置 `WAYLAND_DISPLAY`** 的情况下启动，以避免与 `keytao-installer` 冲突：

```bash
unset WAYLAND_DISPLAY
exec keytao-ime
```

#### niri（Wayland compositor）配置示例

```nix
programs.niri.settings = {
  spawn-at-startup = [
    { command = [ "keytao-installer" ]; }
    # X11-only mode: WAYLAND_DISPLAY must be unset so keytao-ime does not try
    # to register a second zwp_input_method_v2 (only one allowed per seat).
    {
      command = [ "sh" "-c" "unset WAYLAND_DISPLAY; exec keytao-ime" ];
    }
  ];

  environment = {
    # Electron / Chromium → native Wayland
    "NIXOS_OZONE_WL" = "1";
    "ELECTRON_OZONE_PLATFORM_HINT" = "wayland";
    # GTK: prefer Wayland, fall back to X11
    "GDK_BACKEND" = "wayland,x11";
    # Qt: prefer Wayland, fall back to xcb (XWayland)
    "QT_QPA_PLATFORM" = "wayland;xcb";
    # XWayland display — niri always assigns :0.
    # Required so startup processes (e.g. keytao-ime) can connect to XWayland
    # even before the first X11 client triggers its lazy initialization.
    "DISPLAY" = ":0";
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

Linux 打包（deb + tar.gz）：

```bash
pnpm build:linux
```

Android 构建需要配置 Android SDK 与 NDK，参考 [Tauri 移动端文档](https://tauri.app/start/prerequisites/#android)。

## 许可证

MIT
