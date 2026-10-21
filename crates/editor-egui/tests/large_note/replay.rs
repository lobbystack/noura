//! Replays a keystroke script in an editor the way eframe drives one: each
//! frame runs egui with that frame's input, tessellates, and when a wgpu
//! adapter exists renders through egui-wgpu, glyphon's paint callback
//! included, into an offscreen target and waits for the GPU to finish.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use editor_egui::{Editor, MdConfig};
use egui::{Pos2, RawInput, Rect, Vec2, ViewportId, ViewportInfo};
use egui_wgpu::wgpu;
use glyphon::{FontSystem, fontdb};

use crate::script::{Goto, Step};

/// The example's window size.
pub const WINDOW: Vec2 = Vec2::new(900.0, 700.0);

/// A Retina display's scale.
pub const RETINA: f32 = 2.0;

/// What the GPU needs to render a frame.
struct Gpu {
    render_state: egui_wgpu::RenderState,
    view: wgpu::TextureView,
    size: [u32; 2],
}

impl Gpu {
    /// A render state on the machine's default adapter with an offscreen
    /// target of `size` pixels, or `None` without an adapter.
    fn new(size: [u32; 2], font_system: Arc<Mutex<FontSystem>>) -> Option<Self> {
        let setup = egui_kittest::wgpu::default_wgpu_setup();
        let instance = pollster::block_on(setup.new_instance());
        let render_state = pollster::block_on(egui_wgpu::RenderState::create(
            &egui_wgpu::WgpuConfiguration {
                wgpu_setup: setup,
                ..Default::default()
            },
            &instance,
            None,
            egui_wgpu::RendererOptions::default(),
        ))
        .ok()?;
        editor_egui::register_render_callback_resources(
            &render_state.device,
            &render_state.queue,
            render_state.target_format,
            &mut render_state.renderer.write(),
            font_system,
            1,
        );
        let texture = render_state
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("replay target"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: render_state.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Some(Self {
            render_state,
            view,
            size,
        })
    }

    /// Uploads, renders and submits a frame, then waits for the GPU.
    fn render(&self, ctx: &egui::Context, output: egui::FullOutput) {
        let state = &self.render_state;
        let mut renderer = state.renderer.write();
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(&state.device, &state.queue, *id, delta);
            }
        }
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: self.size,
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let callbacks = renderer.update_buffers(
            &state.device,
            &state.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("replay pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            renderer.render(&mut pass, &primitives, &screen);
        }
        state
            .queue
            .submit(callbacks.into_iter().chain([encoder.finish()]));
        state
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("the GPU finishes the frame");
        for id in &output.textures_delta.free {
            renderer.free_texture(id);
        }
    }
}

/// One timed keystroke.
#[derive(Clone, Debug)]
pub struct Sample {
    /// The keystroke's step kind: `type`, `Enter`, `paste`, ...
    pub kind: String,
    /// How many gotos came before it, starting at 1.
    pub place: usize,
    /// From the event's arrival to the end of the frame that showed it.
    pub latency: Duration,
}

/// An editor and the loop that drives its frames.
pub struct Replay {
    ctx: egui::Context,
    pub editor: Editor,
    gpu: Option<Gpu>,
    /// The display's scale, which the platform reports with every frame.
    pixels_per_point: f32,
    started: Instant,
}

/// Which fonts the editor shapes with.
pub enum Fonts {
    /// egui's bundled fonts.
    Bundled,
    /// egui's bundled fonts, then the system's, as the example loads them.
    WithSystem,
}

impl Replay {
    /// An editor on `text` in a window of [`WINDOW`] at `pixels_per_point`.
    /// It renders on the GPU when `gpu` is set and an adapter exists.
    pub fn open(text: &str, fonts: Fonts, pixels_per_point: f32, gpu: bool) -> Self {
        let mut db = editor_egui::fonts::default_font_database();
        if matches!(fonts, Fonts::WithSystem) {
            db.load_system_fonts();
        }
        Self::open_with(text, db, pixels_per_point, gpu)
    }

    fn open_with(text: &str, db: fontdb::Database, pixels_per_point: f32, gpu: bool) -> Self {
        let font_system = Arc::new(Mutex::new(FontSystem::new_with_locale_and_db(
            "en-US".into(),
            db,
        )));
        let ctx = egui::Context::default();
        editor_egui::install_font_system(&ctx, Arc::clone(&font_system));
        let size = WINDOW * pixels_per_point;
        let gpu = gpu
            .then(|| Gpu::new([size.x as u32, size.y as u32], font_system))
            .flatten();
        let editor = Editor::new(&ctx, text, MdConfig::default());
        Self {
            ctx,
            editor,
            gpu,
            pixels_per_point,
            started: Instant::now(),
        }
    }

    /// Whether frames render on the GPU.
    pub fn renders_on_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    fn input(&self, events: Vec<egui::Event>) -> RawInput {
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
            time: Some(self.started.elapsed().as_secs_f64()),
            focused: true,
            events,
            ..Default::default()
        };
        input.viewports.insert(
            ViewportId::ROOT,
            ViewportInfo {
                native_pixels_per_point: Some(self.pixels_per_point),
                focused: Some(true),
                ..Default::default()
            },
        );
        input
    }

    /// Runs one frame with `events` as its input. Returns how long the frame
    /// took to reach the screen and whether it asked for another frame
    /// right away.
    pub fn frame(&mut self, events: Vec<egui::Event>) -> (Duration, bool) {
        let start = Instant::now();
        let input = self.input(events);
        let editor = &mut self.editor;
        let mut output = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                editor.show(ui);
            });
        });
        let again = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .is_some_and(|viewport| viewport.repaint_delay.is_zero());
        match &self.gpu {
            Some(gpu) => gpu.render(&self.ctx, output),
            None => {
                let shapes = std::mem::take(&mut output.shapes);
                let _ = self.ctx.tessellate(shapes, output.pixels_per_point);
                output.drop_without_applying_deltas();
            }
        }
        (start.elapsed(), again)
    }

    /// Runs frames until the editor stops asking for one, at most `limit`.
    /// Returns how many ran.
    pub fn settle(&mut self, limit: usize) -> usize {
        for ran in 1..=limit {
            if !self.frame(Vec::new()).1 {
                return ran;
            }
        }
        limit
    }

    /// Puts the caret at a script's goto target and lets the editor scroll
    /// to it.
    pub fn goto(&mut self, goto: &Goto) {
        let offset = goto
            .offset(self.editor.text())
            .unwrap_or_else(|| panic!("{goto:?} isn't in the note"));
        self.editor.open_navigate(offset..offset);
        self.settle(5);
    }

    /// Replays `steps`, timing each keystroke.
    pub fn run(&mut self, steps: &[Step]) -> Vec<Sample> {
        let mut samples = Vec::new();
        let mut place = 0;
        for step in steps {
            match step {
                Step::Goto(goto) => {
                    self.goto(goto);
                    place += 1;
                }
                Step::Keys { kind, events } => {
                    for event in events {
                        let (latency, _) = self.frame(vec![event.clone()]);
                        samples.push(Sample {
                            kind: kind.clone(),
                            place,
                            latency,
                        });
                        self.settle(3);
                    }
                }
            }
        }
        samples
    }
}
