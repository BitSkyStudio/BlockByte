mod atlas;
mod game;
mod render;
mod ui;

use std::{env::args, net::SocketAddr, path::Path, sync::Arc};

use block_byte_common::{Color, registry::load_registries};
use simple_game_engine::{GameEngineInterface, GameState, run_game};
use winit::{
    dpi::PhysicalPosition,
    window::{CursorGrabMode, Fullscreen, Window},
};

use crate::{
    atlas::init_texture_atlas,
    game::{ClientConnection, ClientConnectionState, ClientGame, profiler},
    render::{GUIMesh, RenderState},
    ui::{UIPos, text_renderer},
};

fn main() {
    load_registries(&[&Path::new("assets"), &Path::new("assets_generated")]);
    init_texture_atlas();

    /*rayon::ThreadPoolBuilder::new()
    .num_threads(8)
    .build_global();*/

    run_game::<BBGame>();
}

pub struct BBGame {
    render_state: RenderState,
    screen: Box<dyn GameScreen>,
}
impl GameState for BBGame {
    fn start(
        gpu: Arc<simple_gpu::GPU>,
        desc_heap: &mut simple_gpu::GPUDescriptorHeap,
        window: &Window,
    ) -> Self {
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        set_cursor_visible(window, false);
        let mut render_state = RenderState::new(
            gpu,
            desc_heap,
            image::load_from_memory(std::fs::read("assets/skybox.png").unwrap().as_slice())
                .unwrap()
                .to_rgba8(),
        );
        let mut screen = Box::new(ConnectionScreen::new(
            match args().nth(1) {
                Some(addr) => {
                    println!("{}", addr);
                    addr
                }
                None => "127.0.0.1:5000".to_string(),
            }
            .parse()
            .unwrap(),
        ));
        screen.start(&mut render_state);
        BBGame {
            render_state,
            screen,
        }
    }

    fn frame(&mut self, mut engine: simple_game_engine::GameEngineInterface<'_>) {
        let mut screen_transition = None;
        self.render_state.tick(&mut engine);
        self.screen
            .render(&mut engine, &mut self.render_state, &mut screen_transition);
        if let Some(screen_transition) = screen_transition {
            self.screen.exit(&mut self.render_state);
            self.screen = screen_transition;
            self.screen.start(&mut self.render_state);
        }
        let profiler_data = profiler::profiler_consume();
        if 1000. * engine.dt > 16. && false {
            println!("slow frame");
            for (name, duration) in profiler_data {
                println!("\t{} - {}", name, duration.as_micros());
            }
        }
    }
}

trait GameScreen {
    fn start(&mut self, renderer: &mut RenderState);
    fn render(
        &mut self,
        engine: &mut GameEngineInterface,
        renderer: &mut RenderState,
        screen_transition: &mut Option<Box<dyn GameScreen>>,
    );
    fn exit(&mut self, renderer: &mut RenderState);
}

struct ConnectionScreen {
    pub connection: Option<ClientConnection>,
}
impl ConnectionScreen {
    pub fn new(address: SocketAddr) -> ConnectionScreen {
        let connection = ClientConnection::connect(address);
        ConnectionScreen {
            connection: Some(connection),
        }
    }
}
impl GameScreen for ConnectionScreen {
    fn render(
        &mut self,
        engine: &mut GameEngineInterface,
        renderer: &mut RenderState,
        screen_transition: &mut Option<Box<dyn GameScreen>>,
    ) {
        match self.connection.as_ref().unwrap().state() {
            ClientConnectionState::Connecting => {}
            ClientConnectionState::Connected => {
                *screen_transition =
                    Some(Box::new(ClientGame::new(self.connection.take().unwrap())));
            }
            ClientConnectionState::Disconnect => {
                unreachable!()
            }
            ClientConnectionState::Disconnected => {
                panic!("disconnected")
            }
        }
        let mut gui_mesh = GUIMesh::default();
        let text = "connecting";
        let text_size = text_renderer().get_size(text, 0.05);
        text_renderer().draw(
            UIPos {
                x: -text_size.x / 2.,
                y: 0.,
            },
            text,
            0.05,
            Color::WHITE,
            &mut gui_mesh,
        );
        renderer.render_gui(engine, gui_mesh, true);
    }
    fn exit(&mut self, _renderer: &mut RenderState) {
        if let Some(connection) = &self.connection {
            *connection.state.lock() = ClientConnectionState::Disconnect;
        }
    }
    fn start(&mut self, _renderer: &mut RenderState) {}
}

pub fn set_cursor_visible(window: &Window, visible: bool) {
    window.set_cursor_visible(visible);
    let _ = window.set_cursor_grab(if visible {
        CursorGrabMode::None
    } else {
        CursorGrabMode::Confined
    });
    if visible {
        let _ = window.set_cursor_position(PhysicalPosition::new(
            window.inner_size().width / 2,
            window.inner_size().height / 2,
        ));
    }
}
