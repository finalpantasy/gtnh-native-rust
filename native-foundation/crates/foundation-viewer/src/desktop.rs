//! Native window integration. Verification uses an explicitly hidden window and a fixed-step script.
mod panel;
use foundation_lab::{
    gpu::{Gpu, write_png},
    session::{Session, Status},
    world_scene::Scene,
    world_storage::Backend,
};
use foundation_viewer::{
    camera::{Camera, GridPoint},
    input::{Action, Input, Key},
    stepper::Stepper,
};
use serde_json::json;
use std::{
    env, fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use voxel_core::coords::CubePos;
use voxel_core::raycast::{Hit, Outcome};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};
type Error = Box<dyn std::error::Error>;

#[derive(Clone)]
struct Args {
    verify: bool,
    capture: PathBuf,
    report: PathBuf,
    resume: Option<PathBuf>,
}
impl Args {
    fn parse() -> Result<Self, Error> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../evidence");
        let mut result = Self {
            verify: false,
            capture: base.join("native-viewer.png"),
            report: base.join("native-viewer.json"),
            resume: None,
        };
        let mut args = env::args().skip(1);
        let mut fresh = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--verify" => result.verify = true,
                "--new" => fresh = true,
                "--capture" => {
                    result.capture = PathBuf::from(args.next().ok_or("--capture requires a path")?)
                }
                "--report" => {
                    result.report = PathBuf::from(args.next().ok_or("--report requires a path")?)
                }
                "--resume" => {
                    result.resume = Some(PathBuf::from(
                        args.next().ok_or("--resume requires an owned world path")?,
                    ))
                }
                _ => return Err(format!("unknown viewer argument: {arg}").into()),
            }
        }
        if fresh && result.resume.is_some() {
            return Err("choose --new or --resume".into());
        }
        if result.verify && result.resume.is_some() {
            return Err(
                "verification always uses a new isolated world; --resume is for manual play".into(),
            );
        }
        if !result.verify && !fresh && result.resume.is_none() {
            let pointer = player_pointer();
            if pointer.exists() {
                let info = fs::symlink_metadata(&pointer)?;
                if !info.file_type().is_file() || info.len() > 4096 {
                    return Err("invalid last-lab pointer; files preserved".into());
                }
                let value: serde_json::Value = serde_json::from_slice(&fs::read(pointer)?)?;
                if value["schema_version"] != 1 {
                    return Err("unknown last-lab pointer schema; files preserved".into());
                }
                result.resume = Some(PathBuf::from(
                    value["world_directory"]
                        .as_str()
                        .ok_or("last-lab pointer is missing its world")?,
                ));
            }
        }
        Ok(result)
    }
}

fn player_pointer() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.local/foundation-viewer/last-world.json")
}
fn remember_player_world(args: &Args, session: &Session) -> Result<(), Error> {
    if args.verify {
        return Ok(());
    }
    use std::io::Write;
    let pointer = player_pointer();
    fs::create_dir_all(pointer.parent().ok_or("last-lab pointer has no parent")?)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = pointer.with_extension(format!("{}-{stamp}.partial", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(
        &json!({"schema_version":1,"world_directory":session.root()}),
    )?)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, pointer)?;
    Ok(())
}

fn trace(args: &Args, message: &str) -> Result<(), Error> {
    if args.verify {
        use std::io::Write;
        if let Some(parent) = args.report.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(args.report.with_extension("trace.log"))?;
        writeln!(log, "pid={} {message}", std::process::id())?;
    }
    Ok(())
}

struct Live {
    args: Args,
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    ui_target: wgpu::Texture,
    scene: Option<Scene>,
    egui: egui::Context,
    egui_state: egui_winit::State,
    ui_renderer: egui_wgpu::Renderer,
    world_image: egui::TextureId,
    session: Option<Session>,
    camera: Camera,
    input: Input,
    clock: Stepper,
    running: bool,
    last_frame: Instant,
    frame: u64,
    message: String,
    text_scale: f32,
    away_energy: Option<u64>,
    pause_checks: u64,
    reopen_progress: Option<u64>,
    presentations: u64,
    occluded_frames: u64,
    inspected: Option<Hit>,
    world_rect: Option<egui::Rect>,
    pointer_inspection_checked: bool,
}
impl Live {
    fn new(event_loop: &ActiveEventLoop, args: Args) -> Result<Self, Error> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Native Foundation Lab")
                    .with_inner_size(PhysicalSize::new(1280, 900))
                    .with_min_inner_size(PhysicalSize::new(960, 680))
                    .with_visible(!args.verify),
            )?,
        );
        if args.verify && window.is_visible() != Some(false) {
            return Err("hidden verification window visibility could not be confirmed".into());
        }
        trace(&args, "window-created hidden=true")?;
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::VULKAN | wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        trace(&args, "adapter-selected")?;
        let caps = surface.get_capabilities(&adapter);
        if !caps.usages.contains(wgpu::TextureUsages::COPY_DST) {
            return Err("native lab surface does not support the required copy destination".into());
        }
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .ok_or("no supported 8-bit lab surface format")?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("no native surface configuration")?;
        config.format = format;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST;
        let scene = Scene::with_gpu(Gpu::from_adapter(&adapter)?)?;
        surface.configure(&scene.gpu().device, &config);
        trace(&args, "scene-and-surface-configured")?;
        let ui_target = Self::target(&scene, &config);
        let egui = egui::Context::default();
        egui.set_visuals(egui::Visuals::dark());
        egui.set_zoom_factor(1.);
        egui.style_mut_of(egui::Theme::Dark, |style| {
            for (text, size) in [
                (egui::TextStyle::Body, 16.),
                (egui::TextStyle::Button, 16.),
                (egui::TextStyle::Small, 14.),
                (egui::TextStyle::Heading, 22.),
            ] {
                style
                    .text_styles
                    .insert(text, egui::FontId::proportional(size));
            }
            style.spacing.item_spacing = egui::vec2(8., 6.);
        });
        let egui_state = egui_winit::State::new(
            egui.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            Some(4096),
        );
        let mut ui_renderer = egui_wgpu::Renderer::new(
            &scene.gpu().device,
            format,
            egui_wgpu::RendererOptions {
                dithering: false,
                ..Default::default()
            },
        );
        let world_image = ui_renderer.register_native_texture(
            &scene.gpu().device,
            &scene.color_texture().create_view(&Default::default()),
            wgpu::FilterMode::Nearest,
        );
        trace(&args, "interface-renderer-created")?;
        let session = if let Some(root) = &args.resume {
            Session::resume(root)?
        } else {
            Session::new(Backend::Paged)?
        };
        remember_player_world(&args, &session)?;
        trace(&args, "session-created; waiting-for-frame")?;
        Ok(Self {
            args,
            window,
            instance,
            surface,
            config,
            ui_target,
            scene: Some(scene),
            egui,
            egui_state,
            ui_renderer,
            world_image,
            session: Some(session),
            camera: Camera::home(),
            input: Input::default(),
            clock: Stepper::default(),
            running: false,
            last_frame: Instant::now(),
            frame: 0,
            message: "Run the factory, then submit 12 ingots.".into(),
            text_scale: 1.,
            away_energy: None,
            pause_checks: 0,
            reopen_progress: None,
            presentations: 0,
            occluded_frames: 0,
            inspected: None,
            world_rect: None,
            pointer_inspection_checked: false,
        })
    }
    fn target(scene: &Scene, config: &wgpu::SurfaceConfiguration) -> wgpu::Texture {
        scene.gpu().device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native lab interface target"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }
    fn session(&mut self) -> Result<&mut Session, Error> {
        self.session
            .as_mut()
            .ok_or_else(|| "lab session unavailable".into())
    }
    fn resize(&mut self, size: PhysicalSize<u32>) -> Result<(), Error> {
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        if size.width > 8192 || size.height > 8192 {
            return Err("native lab window exceeds its 8192-pixel dimension limit".into());
        }
        self.config.width = size.width;
        self.config.height = size.height;
        let scene = self.scene.as_ref().ok_or("renderer unavailable")?;
        self.surface.configure(&scene.gpu().device, &self.config);
        self.ui_target = Self::target(scene, &self.config);
        Ok(())
    }
    fn action(&mut self, action: Action) -> Result<bool, Error> {
        match action {
            Action::ToggleRun => {
                self.running = !self.running;
                self.message = if self.running {
                    "Factory running."
                } else {
                    "Factory paused."
                }
                .into();
            }
            Action::Step => {
                self.running = false;
                let tick = self.session()?.tick()?;
                self.message = format!(
                    "One tick: {} machine(s), {} EU.",
                    tick.machines, tick.energy_used
                );
            }
            Action::Submit => {
                let submitted = self.session()?.submit()?;
                self.message = if submitted.reward_tokens > 0 {
                    "Quest complete. Two lab tokens delivered once.".into()
                } else if submitted.items > 0 {
                    "Submitted one ingot.".into()
                } else if self.session()?.status()?.reward_claimed {
                    "Quest already complete. Reward is safe.".into()
                } else {
                    "No finished ingot is ready to submit.".into()
                };
            }
            Action::Home => {
                self.camera = Camera::home();
                self.message = "Returned to the lab.".into();
            }
            Action::Save => {
                self.session()?.save()?;
                self.message = "Lab saved.".into();
            }
            Action::Quit => {
                self.session()?.save()?;
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn reload(&mut self) -> Result<(), Error> {
        let root = self.session()?.root().to_path_buf();
        self.session()?.save()?;
        drop(self.session.take());
        let mut session = Session::resume(&root)?;
        let progress = session.status()?.submitted;
        if self.args.verify {
            assert_eq!(progress, 6);
            self.reopen_progress = Some(progress);
        }
        self.session = Some(session);
        self.clock = Stepper::default();
        self.message = "Reopened the saved lab.".into();
        Ok(())
    }
    fn scripted_actions(&mut self) -> Result<(), Error> {
        match self.frame {
            10 => {
                self.action(Action::ToggleRun)?;
            }
            25 => {
                self.away_energy = Some(self.session()?.status()?.energy_used);
                self.camera
                    .set_focus(GridPoint::new(CubePos::new(-1, -1_000_000, 0), [8.; 3])?);
            }
            35 => {
                self.action(Action::Home)?;
            }
            140..=145 | 170..=180 => {
                self.action(Action::Submit)?;
            }
            155 => {
                self.running = false;
                self.action(Action::Save)?;
            }
            160 => self.reload()?,
            _ => {}
        }
        Ok(())
    }
    fn movement(&mut self, dt: Duration) -> Result<(), Error> {
        let amount = dt.as_secs_f64().min(0.1) * 16.;
        let axis = |positive, negative| {
            i32::from(self.input.held(positive)) - i32::from(self.input.held(negative))
        };
        self.camera.pan(
            f64::from(axis(Key::D, Key::A)) * amount,
            f64::from(axis(Key::W, Key::S)) * amount,
            f64::from(axis(Key::Space, Key::Control)) * amount,
        )?;
        self.camera.orbit(
            axis(Key::Right, Key::Left) as f32 * amount as f32 * 0.06,
            axis(Key::Up, Key::Down) as f32 * amount as f32 * 0.06,
        )?;
        let zoom = axis(Key::Plus, Key::Minus);
        if zoom != 0 {
            self.camera
                .zoom(zoom as f32 * dt.as_secs_f32().min(0.1) * 6.)?;
        }
        Ok(())
    }
    fn draw(&mut self, event_loop: &ActiveEventLoop) -> Result<bool, Error> {
        if self.frame.is_multiple_of(20) {
            trace(&self.args, &format!("draw-frame={}", self.frame))?;
        }
        let now = Instant::now();
        let elapsed = if self.args.verify {
            Duration::from_millis(50)
        } else {
            now.saturating_duration_since(self.last_frame)
        };
        self.last_frame = now;
        if self.args.verify {
            self.scripted_actions()?;
        }
        self.movement(elapsed)?;
        let focus = self.camera.focus().cube();
        if self.session()?.focus() != focus {
            self.session()?.set_focus(focus)?;
            self.inspected = None;
        }
        self.session()?.pump()?;
        for _ in 0..self.clock.advance(elapsed, self.running) {
            self.session()?.tick()?;
        }
        let status = self.session()?.status()?;
        if self.args.verify && (25..35).contains(&self.frame) {
            assert_eq!(Some(status.energy_used), self.away_energy);
            assert!(status.machines.iter().all(|m| !m.simulated));
            self.pause_checks += 1;
        }
        self.scene
            .as_mut()
            .ok_or("renderer unavailable")?
            .render_camera(
                self.session.as_ref().ok_or("session unavailable")?.world(),
                focus,
                self.camera.matrix(),
            )?;
        let mut raw = self.egui_state.take_egui_input(&self.window);
        if self.args.verify {
            raw.time = Some(self.frame as f64 * 0.05);
            if matches!(self.frame, 184 | 185) {
                let point = self
                    .camera
                    .project_local([8.5, 19.5, 8.5])
                    .ok_or("inspection test target outside camera")?;
                let rect = self.world_rect.ok_or("world image rectangle unavailable")?;
                let pos = egui::pos2(
                    rect.left() + (point[0] + 1.) * 0.5 * rect.width(),
                    rect.top() + (1. - point[1]) * 0.5 * rect.height(),
                );
                raw.events.push(egui::Event::PointerMoved(pos));
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: self.frame == 184,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
        let context = self.egui.clone();
        let mut actions = Vec::new();
        let output = context.run_ui(raw, |ui| self.interface(ui, &status, &mut actions));
        if self.args.verify && self.frame == 188 {
            let hit = self
                .inspected
                .ok_or("native pointer click did not inspect a rendered block")?;
            let world = self.session.as_ref().ok_or("session unavailable")?.world();
            assert!(world.renders(hit.position.cube()));
            assert_eq!(
                world
                    .record(hit.position.cube())
                    .ok_or("inspected cube not resident")?
                    .blocks()[hit.position.local_index()],
                hit.block
            );
            assert!(hit.block > 0 && hit.distance <= 500.);
            self.pointer_inspection_checked = true;
        }
        if self.frame == 0
            && !self.egui.fonts_mut(|fonts| {
                fonts.has_glyphs(
                    &egui::FontId::proportional(16.),
                    "Native Foundation Lab 0123456789 EU / -+[]=—",
                )
            })
        {
            return Err("interface fonts do not contain required label glyphs".into());
        }
        self.egui_state.handle_platform_output_with_event_loop(
            &self.window,
            event_loop,
            output.platform_output,
        );
        let paint = self.egui.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: output.pixels_per_point,
        };
        let scene = self.scene.as_ref().ok_or("renderer unavailable")?;
        let gpu = scene.gpu();
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.ui_renderer
                    .update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let extra =
            self.ui_renderer
                .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &paint, &screen);
        {
            let view = self.ui_target.create_view(&Default::default());
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native lab interface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.035,
                            g: 0.04,
                            b: 0.05,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.ui_renderer
                .render(&mut pass.forget_lifetime(), &paint, &screen);
        }
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.occluded_frames += 1;
                None
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&gpu.device, &self.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.window.clone())?;
                self.surface.configure(&gpu.device, &self.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("native surface validation error".into());
            }
        };
        if let Some(frame) = &surface_texture {
            encoder.copy_texture_to_texture(
                self.ui_target.as_image_copy(),
                frame.texture.as_image_copy(),
                self.ui_target.size(),
            );
        }
        gpu.queue
            .submit(extra.into_iter().chain([encoder.finish()]));
        for id in &output.textures_delta.free {
            self.ui_renderer.free_texture(id);
        }
        let complete = self.args.verify && self.frame == 190;
        if complete {
            self.capture(&status)?;
        }
        if let Some(frame) = surface_texture {
            self.scene.as_ref().unwrap().gpu().queue.present(frame);
            self.presentations += 1;
        }
        let mut quit = false;
        for action in actions {
            match self.action(action) {
                Ok(value) => quit |= value,
                Err(error) => self.message = error.to_string(),
            }
        }
        self.frame += 1;
        Ok(complete || quit)
    }
    fn capture(&mut self, status: &Status) -> Result<(), Error> {
        assert_eq!(status.submitted, 12);
        assert_eq!(status.reward_tokens, 2);
        assert!(status.reward_claimed);
        assert_eq!(status.material_units, 24);
        assert_eq!(status.energy_used, 1824);
        assert_eq!(self.pause_checks, 10);
        assert_eq!(self.reopen_progress, Some(6));
        if let Some(parent) = self.args.capture.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut pixels = self
            .scene
            .as_ref()
            .ok_or("renderer unavailable")?
            .gpu()
            .read_texture(&self.ui_target)?;
        if self.config.format == wgpu::TextureFormat::Bgra8Unorm {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        let white = pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 180 && p[1] > 180 && p[2] > 180)
            .count();
        let green = pixels
            .chunks_exact(4)
            .filter(|p| {
                u16::from(p[1]) > u16::from(p[0]) * 6 / 5
                    && u16::from(p[1]) > u16::from(p[2]) * 11 / 10
                    && p[1] > 40
            })
            .count();
        assert!(
            white > 2000 && green > 3000,
            "interface text or voxel world is missing from capture"
        );
        write_png(
            &self.args.capture,
            self.config.width,
            self.config.height,
            &pixels,
        )?;
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Error> {
        self.session()?.save()?;
        if self.args.verify {
            if self.frame < 191 {
                return Err("verification ended before its scripted route completed".into());
            }
            let status = self.session()?.status()?;
            assert!(
                self.pointer_inspection_checked,
                "native pointer inspection route incomplete"
            );
            let root = self.session()?.root().to_path_buf();
            let render = self
                .scene
                .take()
                .ok_or("renderer already closed")?
                .finish(&self.args.capture)?;
            let report = json!({"status":"CHECKS_PASSED_AWAITING_EXIT","hidden_window":self.window.is_visible()==Some(false),"interface_glyphs_checked":true,"frames":self.frame,"surface_presentations":self.presentations,"occluded_frames":self.occluded_frames,
                "paused_outside_simulation_checks":self.pause_checks,"mid_submission_reopen_progress":self.reopen_progress,"state":status,"render":render,"image":self.args.capture,"world_directory":root,
                "pointer_inspection_checked":self.pointer_inspection_checked,"inspected_block":self.inspected.map(|h| json!({"block":h.block,"cell":[h.position.x,h.position.y,h.position.z],"normal":h.face.map(|f|f.normal()),"distance":h.distance})),
                "fixed_interface_target_bytes":u64::from(self.config.width)*u64::from(self.config.height)*4,"scope":"real hidden native window and shared GPU renderer; deterministic logical input script with 50 ms virtual frames; keyboard mapping separately unit-tested; not a physical keyboard/pointer playtest or real-time performance benchmark; UI textures/targets and driver overhead outside world mesh budget"});
            if let Some(parent) = self.args.report.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&self.args.report, serde_json::to_vec_pretty(&report)?)?;
        }
        Ok(())
    }
}

struct App {
    args: Args,
    live: Option<Live>,
    error: Option<String>,
    next_redraw: Instant,
    closing: bool,
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing || self.live.is_some() {
            return;
        }
        match Live::new(event_loop, self.args.clone()) {
            Ok(live) => self.live = Some(live),
            Err(error) => {
                self.error = Some(error.to_string());
                self.closing = true;
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.closing {
            return;
        }
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.window.id() != id {
            return;
        }
        let response = live.egui_state.on_window_event(&live.window, &event);
        let result: Result<bool, Error> = match event {
            WindowEvent::CloseRequested => live.action(Action::Quit),
            WindowEvent::Resized(size) => live.resize(size).map(|_| false),
            WindowEvent::Focused(false) => {
                live.input.focus_lost();
                if !live.args.verify {
                    live.running = false;
                }
                Ok(false)
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(key) = key(code) {
                        if let Some(action) = live.input.key(
                            key,
                            event.state == ElementState::Pressed,
                            event.repeat,
                            response.consumed,
                        ) {
                            match live.action(action) {
                                Ok(quit) => Ok(quit),
                                Err(error) => {
                                    live.message = error.to_string();
                                    Ok(false)
                                }
                            }
                        } else {
                            Ok(false)
                        }
                    } else {
                        Ok(false)
                    }
                } else {
                    Ok(false)
                }
            }
            WindowEvent::RedrawRequested if !live.args.verify => live.draw(event_loop),
            _ => Ok(false),
        };
        match result {
            Ok(true) => {
                self.closing = true;
                if let Err(error) = live.finish() {
                    self.error = Some(error.to_string());
                }
                event_loop.exit();
            }
            Ok(false) => {}
            Err(error) => {
                self.closing = true;
                self.error = Some(error.to_string());
                event_loop.exit();
            }
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing {
            return;
        }
        if let Some(live) = self.live.as_mut() {
            let now = Instant::now();
            if now >= self.next_redraw {
                if self.args.verify {
                    // Hidden Win32 windows do not receive WM_PAINT: drive the same draw path from the event-loop timer.
                    match live.draw(event_loop) {
                        Ok(true) => {
                            self.closing = true;
                            if let Err(error) = live.finish() {
                                self.error = Some(error.to_string());
                            }
                            event_loop.exit();
                        }
                        Ok(false) => {}
                        Err(error) => {
                            self.closing = true;
                            self.error = Some(error.to_string());
                            event_loop.exit();
                        }
                    }
                } else {
                    live.window.request_redraw();
                }
                self.next_redraw =
                    now + Duration::from_millis(if self.args.verify { 1 } else { 16 });
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_redraw));
        }
    }
}
fn key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::KeyW => Key::W,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyD => Key::D,
        KeyCode::Space => Key::Space,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::ArrowLeft => Key::Left,
        KeyCode::ArrowRight => Key::Right,
        KeyCode::ArrowUp => Key::Up,
        KeyCode::ArrowDown => Key::Down,
        KeyCode::Equal | KeyCode::NumpadAdd => Key::Plus,
        KeyCode::Minus | KeyCode::NumpadSubtract => Key::Minus,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyE => Key::E,
        KeyCode::Home => Key::Home,
        KeyCode::Escape => Key::Escape,
        _ => return None,
    })
}
pub fn run() -> Result<(), Error> {
    let args = Args::parse()?;
    let event_loop = EventLoop::new()?;
    let mut app = App {
        args: args.clone(),
        live: None,
        error: None,
        next_redraw: Instant::now(),
        closing: false,
    };
    event_loop.run_app(&mut app)?;
    let error = app.error.take();
    drop(app);
    if let Some(error) = error {
        if args.verify {
            fs::write(
                &args.report,
                serde_json::to_vec_pretty(&json!({"status":"FAIL","error":error}))?,
            )?;
        }
        return Err(error.into());
    }
    if args.verify {
        let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&args.report)?)?;
        if report["status"] != "CHECKS_PASSED_AWAITING_EXIT" {
            return Err("missing current native verification receipt".into());
        }
        report["status"] = json!("PASS");
        report["clean_event_loop_exit"] = json!(true);
        fs::write(&args.report, serde_json::to_vec_pretty(&report)?)?;
        println!(
            "{}",
            json!({"status":"PASS","frames":report["frames"],"submitted":report["state"]["submitted"],"reward_tokens":report["state"]["reward_tokens"],"image":args.capture,"report":args.report})
        );
    }
    Ok(())
}
