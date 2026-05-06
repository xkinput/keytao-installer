# keytao-linux-ime

Standalone Linux IME binary for KeyTao. No IBus or Fcitx5 required.
Works directly over Wayland (`zwp_input_method_v2`) and X11 (XIM protocol).

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        keytao-linux-ime                             │
│                                                                     │
│  main.rs                                                            │
│  ├─ init CoreEngine (deploy librime, load schemas)                  │
│  ├─ detect display server                                           │
│  │    WAYLAND_DISPLAY set? ──► wayland_backend::run()              │
│  │    DISPLAY set?         ──► x11_backend::run()                  │
│  └─ load font for panel renderer (NotoSansCJK / wqy / fc-match)    │
│                                                                     │
│  engine.rs  (CoreEngine)                                            │
│  └─ wraps keytao-core::{Engine, deploy}                             │
│       process_key(keycode, mask) → ImeState                         │
│       select_candidate(index)   → ImeState                         │
│       change_page(backward)     → ImeState                         │
│       reset()                   → ImeState                         │
│                                                                     │
│  panel.rs  (PanelRenderer)                                          │
│  └─ renders preedit + candidate bar → BGRA pixel buffer             │
│       font: fontdue (rasterize CJK glyphs at runtime)              │
│       draw: tiny_skia (fill rects, blit glyphs)                    │
│       theme: Catppuccin Mocha (hardcoded dark palette)             │
│                                                                     │
│  wayland_backend.rs                                                 │
│  ├─ zwp_input_method_manager_v2  — register as input method        │
│  ├─ zwp_input_method_v2          — activate/deactivate lifecycle   │
│  ├─ zwp_input_method_keyboard_grab_v2 — exclusive key grab         │
│  ├─ zwp_input_popup_surface_v2   — compositor-positioned panel     │
│  └─ wl_shm                       — upload BGRA buffer to surface   │
│                                                                     │
│  x11_backend.rs                                                     │
│  ├─ XIM server (@server=keytao, set XMODIFIERS=@im=keytao)        │
│  ├─ xim crate (x11rb) — handle IC create/destroy/key events       │
│  └─ XCB overlay window  — upload BGRA buffer via XCBImage         │
└─────────────────────────────────────────────────────────────────────┘
         │                                │
         ▼                                ▼
┌─────────────────┐             ┌──────────────────┐
│  keytao-core    │             │  librime.so       │
│  (Rust wrapper) │────────────►│  (rime engine)    │
│                 │             │  schema files in  │
│  Engine         │             │  ~/.local/share/  │
│  ImeState       │             │  fcitx5/rime/ or  │
│  deploy()       │             │  ~/.config/ibus/  │
└─────────────────┘             │  rime/            │
                                └──────────────────┘
```

## Data flow (key press → commit)

```
App (any GUI app)
  │  key event via Wayland/XIM protocol
  ▼
keytao-linux-ime
  │  keycode + modifier mask
  ▼
CoreEngine::process_key()
  │  forwards to librime via keytao-core
  ▼
ImeState { preedit, candidates, committed, ... }
  │
  ├─► committed text  ──► commit_string() to app
  ├─► preedit text    ──► set_preedit_string() to app
  └─► candidates      ──► PanelRenderer → pixel buffer
                               │
                         Wayland: wl_surface (popup)
                         X11:     XCB overlay window
```

## Wayland setup

The compositor must support `zwp_input_method_v2` (KDE Plasma ≥ 5.24, Sway ≥ 1.7, Wayfire, river).

```sh
# Launch (usually handled by the installer's autostart entry)
keytao-linux-ime
```

## X11 setup

```sh
export XMODIFIERS=@im=keytao
export GTK_IM_MODULE=xim
export QT_IM_MODULE=xim
keytao-linux-ime &
```

## Schema init

On first run, `engine.rs` checks for `default.custom.yaml` in the user data
directory and writes it if missing (enabling keytao / keytao-dz / keytao-bj
schemas with page size 6), then calls `keytao-core::deploy()` to compile the
schema database before starting the event loop.

## Dependencies

| crate | purpose |
|-------|---------|
| `keytao-core` | librime wrapper — Engine, ImeState, deploy |
| `tiny_skia` | software 2D renderer for candidate panel |
| `fontdue` | font rasterizer (no system harfbuzz required) |
| `wayland-client` | Wayland protocol dispatch |
| `wayland-protocols-misc` | `zwp_input_method_v2` protocol definitions |
| `xkbcommon` | keymap + modifier state on Wayland |
| `x11rb` | XCB connection for X11 backend |
| `xim` | XIM server implementation on top of x11rb |
