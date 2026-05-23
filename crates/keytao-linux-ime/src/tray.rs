use ksni::{MenuItem, Tray};
use std::process::Command;

pub struct KeytaoTray;

impl Tray for KeytaoTray {
    fn icon_name(&self) -> String {
        "keytao-app".into()
    }

    fn title(&self) -> String {
        "KeyTao IME".into()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            MenuItem::Standard(ksni::menu::StandardItem {
                label: "Configure KeyTao...".into(),
                icon_name: "preferences-system".into(),
                activate: Box::new(|_| {
                    // Spawn the keytao-app UI
                    Command::new("keytao-app").spawn().ok();
                }),
                ..Default::default()
            }),
            MenuItem::Separator,
            MenuItem::Standard(ksni::menu::StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| {
                    std::process::exit(0);
                }),
                ..Default::default()
            }),
        ]
    }
}

pub fn spawn_tray() {
    std::thread::spawn(|| {
        let service = ksni::TrayService::new(KeytaoTray);
        service.spawn();
    });
}
