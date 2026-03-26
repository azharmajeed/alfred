mod app;
mod platform;
mod renderer;
mod terminal;
mod workspace;

use winit::event_loop::EventLoop;

use app::{App, UserEvent};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);

    event_loop.run_app(&mut app)?;

    Ok(())
}
