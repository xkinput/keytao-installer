//! KeyTao ibus engine for Linux.
//!
//! Architecture:
//!   keytao-ibus (this binary)
//!     ↕  D-Bus  (org.freedesktop.IBus)
//!   ibus-daemon
//!     ↕  X11 / Wayland input protocol
//!   any application
//!
//! The engine registers itself with ibus-daemon via D-Bus.
//! Key events come in through `ProcessKeyEvent`, get forwarded to keytao-core,
//! and the resulting candidates + preedit are sent back as D-Bus signals.
//!
//! To install as a system engine, place the .xml component file in
//! /usr/share/ibus/component/ and the binary in /usr/lib/ibus/.
//!
//! Component XML example (keytao.xml):
//!   <component>
//!     <name>org.freedesktop.IBus.KeyTao</name>
//!     <description>KeyTao Shuang-pin Input Method</description>
//!     <exec>/usr/lib/ibus/ibus-engine-keytao --ibus</exec>
//!     <engines>
//!       <engine>
//!         <name>keytao-bj</name>
//!         <language>zh</language>
//!         <license>MIT</license>
//!         <author>KeyTao Contributors</author>
//!         <layout>default</layout>
//!         <longname>键道6北京</longname>
//!       </engine>
//!     </engines>
//!   </component>

#[cfg(target_os = "linux")]
mod engine;

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    engine::run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("keytao-ibus only runs on Linux");
    std::process::exit(1);
}
