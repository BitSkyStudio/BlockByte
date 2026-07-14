mod atlas;
mod game;
mod render;
mod ui;

use core::f32;
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    path::Path,
    time::Instant,
};

use block_byte_common::{Color, registry::load_registries};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Fullscreen, WindowAttributes, WindowId},
};

use crate::{
    atlas::init_texture_atlas,
    game::{ClientConnection, ClientConnectionState, ClientGame, profiler},
    render::{GUIMesh, RenderState, SurfaceError},
    ui::{UIPos, text_renderer},
};

fn main() {
    load_registries(&[&Path::new("assets"), &Path::new("assets_generated")]);
    init_texture_atlas();

    /*rayon::ThreadPoolBuilder::new()
    .num_threads(8)
    .build_global();*/

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    event_loop
        .run_app(&mut App {
            render_state: None,
            screen: Box::new(ConnectionScreen::new()),
            input: InputManager::default(),
            last_update: Instant::now(),
        })
        .unwrap();
}

trait GameScreen {
    fn render(
        &mut self,
        input: &InputManager,
        renderer: &mut RenderState,
        dt: f32,
        screen_transition: &mut Option<Box<dyn GameScreen>>,
    );
    fn exit(&mut self);
}

struct App {
    render_state: Option<RenderState>,
    screen: Box<dyn GameScreen>,
    input: InputManager,
    last_update: Instant,
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window_attributes = WindowAttributes::default();
        let window = match event_loop.create_window(window_attributes) {
            Ok(window) => window,
            Err(err) => {
                eprintln!("error creating window: {err}");
                event_loop.exit();
                return;
            }
        };
        let _ = window.set_cursor_grab(winit::window::CursorGrabMode::Confined);
        window.set_cursor_visible(false);
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        self.render_state = Some(pollster::block_on(RenderState::new(
            window,
            image::load_from_memory(std::fs::read("assets/skybox.png").unwrap().as_slice())
                .unwrap()
                .to_rgba8(),
        )));
    }
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        match event {
            DeviceEvent::MouseMotion {
                delta: (delta_x, delta_y),
            } => {
                self.input.mouse_delta.x += delta_x;
                self.input.mouse_delta.y += delta_y;
            }
            DeviceEvent::Key(event) => match event.physical_key {
                PhysicalKey::Code(key_code) => {
                    if event.state == ElementState::Pressed {
                        self.input.keys.press(key_code);
                    } else {
                        self.input.keys.release(key_code);
                    }
                }
                PhysicalKey::Unidentified(_) => {}
            },
            _ => {}
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.screen.exit();
                event_loop.exit();
            }
            WindowEvent::CursorMoved {
                device_id: _,
                position,
            } => {
                self.input.cursor_position = UIPos {
                    x: position.x as f32,
                    y: position.y as f32,
                };
            }
            WindowEvent::Resized(new_size) => {
                self.render_state.as_mut().unwrap().resize(new_size);
            }
            WindowEvent::MouseWheel {
                device_id: _,
                delta,
                phase: _,
            } => match delta {
                MouseScrollDelta::LineDelta(scroll_x, scroll_y) => {
                    self.input.wheel_scroll_delta.x += scroll_x;
                    self.input.wheel_scroll_delta.y += scroll_y;
                }
                MouseScrollDelta::PixelDelta(_) => {
                    //todo
                }
            },
            WindowEvent::MouseInput {
                device_id: _,
                state,
                button,
            } => match state {
                ElementState::Pressed => {
                    self.input.buttons.press(button);
                }
                ElementState::Released => {
                    self.input.buttons.release(button);
                }
            },
            WindowEvent::RedrawRequested => {
                let dt = self.last_update.elapsed().as_secs_f32();
                self.last_update = Instant::now();

                self.render_state
                    .as_ref()
                    .unwrap()
                    .window()
                    .pre_present_notify();

                let render_state = self.render_state.as_mut().unwrap();
                render_state.animation_time += dt;
                let mut screen_transition = None;
                self.screen
                    .render(&self.input, render_state, dt, &mut screen_transition);

                if let Some(screen_transition) = screen_transition {
                    self.screen = screen_transition;
                }

                self.render_state
                    .as_ref()
                    .unwrap()
                    .window()
                    .request_redraw();

                self.input.buttons.frame_clear(dt);
                self.input.keys.frame_clear(dt);
                self.input.last_cursor_position = self.input.cursor_position;
                self.input.wheel_scroll_delta = UIPos { x: 0., y: 0. };
                self.input.mouse_delta = PhysicalPosition { x: 0., y: 0. };

                let profiler_data = profiler::profiler_consume();
                if self.last_update.elapsed().as_millis() > 18 && false {
                    println!(
                        "slow frame: {}",
                        self.last_update.elapsed().as_secs_f32() * 1000.
                    );
                    for (name, duration) in profiler_data {
                        println!("\t{} - {}", name, duration.as_micros());
                    }
                }
            }
            _ => (),
        }
    }
}
pub struct InputContainer<T> {
    pub down: HashSet<T>,
    pub just_down: HashSet<T>,
    pub just_up: HashSet<T>,
    pub held_for: HashMap<T, f32>,
}
impl<T> Default for InputContainer<T> {
    fn default() -> Self {
        Self {
            down: HashSet::new(),
            just_down: HashSet::new(),
            just_up: HashSet::new(),
            held_for: HashMap::new(),
        }
    }
}
impl<T: Hash + Eq + Copy> InputContainer<T> {
    pub fn press(&mut self, input: T) {
        self.down.insert(input);
        self.just_down.insert(input);
        self.held_for.insert(input, 0.);
    }
    pub fn release(&mut self, input: T) {
        self.down.remove(&input);
        self.just_up.insert(input);
        self.held_for.remove(&input);
    }
    pub fn frame_clear(&mut self, dt: f32) {
        self.just_down.clear();
        self.just_up.clear();
        for time in self.held_for.values_mut() {
            *time += dt;
        }
    }
    pub fn is_down(&self, input: T) -> bool {
        self.down.contains(&input)
    }
    pub fn is_just_down(&self, input: T) -> bool {
        self.just_down.contains(&input)
    }
    pub fn is_just_up(&self, input: T) -> bool {
        self.just_up.contains(&input)
    }
    pub fn held_time(&self, input: T) -> f32 {
        self.held_for.get(&input).cloned().unwrap_or(0.)
    }
}

#[derive(Default)]
pub struct InputManager {
    pub keys: InputContainer<KeyCode>,
    pub buttons: InputContainer<MouseButton>,
    pub cursor_position: UIPos,
    pub last_cursor_position: UIPos,
    pub wheel_scroll_delta: UIPos,
    pub mouse_delta: PhysicalPosition<f64>,
}

struct ConnectionScreen {
    pub connection: Option<ClientConnection>,
}
impl ConnectionScreen {
    pub fn new() -> ConnectionScreen {
        let connection = ClientConnection::connect("127.0.0.1:5000".parse().unwrap());
        ConnectionScreen {
            connection: Some(connection),
        }
    }
}
impl GameScreen for ConnectionScreen {
    fn render(
        &mut self,
        _input: &InputManager,
        renderer: &mut RenderState,
        _dt: f32,
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
        match renderer.render_gui(gui_mesh) {
            Ok(_) => {}
            Err(SurfaceError::Recreate) => renderer.resize(renderer.size()),
            Err(SurfaceError::Crash) => {
                panic!("crash");
            }
        }
    }
    fn exit(&mut self) {
        if let Some(connection) = &self.connection {
            *connection.state.lock() = ClientConnectionState::Disconnect;
        }
    }
}
