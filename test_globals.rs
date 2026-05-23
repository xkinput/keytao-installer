use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_registry};

struct App;
impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface.contains("input_method") {
                println!("global: {} {} {}", name, interface, version);
            }
        }
    }
}

fn main() {
    let conn = Connection::connect_to_env().unwrap();
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    let _registry = display.get_registry(&qh, ());
    let mut app = App;
    event_queue.roundtrip(&mut app).unwrap();
}
