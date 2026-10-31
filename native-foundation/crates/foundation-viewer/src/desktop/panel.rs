use super::*;
impl Live {
    pub(super) fn interface(
        &mut self,
        ui: &mut egui::Ui,
        status: &Status,
        actions: &mut Vec<Action>,
    ) {
        egui::Panel::left("lab-controls").exact_size(285.).resizable(false).show(ui,|ui| {
            egui::Panel::bottom("save-controls").exact_size(42.).resizable(false).show(ui,|ui| {
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {actions.push(Action::Save);}
                    if ui.button("Save and quit").clicked() {actions.push(Action::Quit);}
                });
            });
            egui::ScrollArea::vertical().show(ui,|ui| {
                ui.heading("Native Foundation Lab");ui.label("Synthetic Rust experiment");ui.separator();
                ui.label("Run, then submit 12 ingots.");
                ui.horizontal(|ui| {
                    if ui.button(if self.running {"Pause [P]"} else {"Run [P]"}).clicked() {actions.push(Action::ToggleRun);}
                    if ui.button("Step [N]").clicked() {actions.push(Action::Step);}
                });
                for(index,machine) in status.machines.iter().enumerate() {
                    ui.add_space(5.);ui.horizontal(|ui| {ui.strong(if index==0 {"Crusher"} else {"Smelter"});ui.label(format!("{} / 12 batches",machine.completed));});
                    let phase=if !machine.simulated {"Outside simulation"} else if machine.completed==12 {"Finished"} else if machine.progress.is_some() {"Processing"} else if self.running {"Waiting for input"} else {"Paused"};ui.small(format!("{phase} · {} EU",machine.energy));
                    if let Some(progress)=machine.progress {let total=if index==0 {5.} else {7.};ui.add(egui::ProgressBar::new(progress as f32/total).text(format!("{progress} / {} ticks",total as u32)));}
                    for(name,count) in &machine.outputs {ui.label(format!("Ready: {count} {}",name.trim_start_matches("lab:").replace('_'," ")));}
                }
                ui.separator();ui.strong("Quest ledger");ui.label(format!("Submitted: {} / 12 ingots",status.submitted));
                ui.label(format!("Reward: {} / 2 tokens{}",status.reward_tokens,if status.reward_claimed {" — claimed"} else {""}));
                let ingots=status.machines.get(1).is_some_and(|m|m.outputs.iter().any(|(id,n)|id=="lab:iron_ingot" && *n>0));
                let ready=ingots && status.machines.iter().all(|m|m.simulated);
                if ui.add_enabled(ready,egui::Button::new("Submit one ingot [E]")).clicked() {actions.push(Action::Submit);}
                ui.separator();ui.strong("Inspect the cubic world");
                ui.small("Left-click a block to inspect it.");
                if let Some(hit)=self.inspected {
                    ui.label(format!("Block {} · {:.2} blocks away",hit.block,hit.distance));
                    ui.small(format!("Cell {}, {}, {}",hit.position.x,hit.position.y,hit.position.z));
                    ui.small(format!("Entry normal: {:?}",hit.face.map(|f|f.normal())));
                }
                if ui.button("Home lab [Home]").clicked() {actions.push(Action::Home);}
                ui.horizontal(|ui| {
                    if ui.button("Up one cube").clicked() && let Err(error)=self.camera.pan(0.,0.,16.) {self.message=error;}
                    if ui.button("Down one cube").clicked() && let Err(error)=self.camera.pan(0.,0.,-16.) {self.message=error;}
                });
                ui.collapsing("Controls",|ui| {ui.label("Click the world for camera keys. WASD pans; Space/Ctrl moves vertically. Right-drag or arrow keys orbits. Wheel or +/- zooms.");ui.label("P runs/pauses, N steps, E submits. Ctrl+S saves. Esc saves and quits.");});
                if ui.add(egui::Slider::new(&mut self.text_scale,0.9 ..=1.6).text("Text scale")).changed() {self.egui.set_zoom_factor(self.text_scale);}
                ui.collapsing("Debug counters",|ui| {
                    ui.label(format!("Cubes: {} / 27",status.resident_cubes));ui.label(format!("Cube data: {:.1} KiB",status.charged_cube_bytes as f64/1024.));
                    if let Some(scene)=&self.scene {let usage=scene.usage();ui.label(format!("World meshes: {:.2} / 1 MiB GPU",usage.gpu() as f64/1048576.));ui.small("UI textures and fixed targets are separate.");}
                    ui.label(format!("Material units: {} / 24",status.material_units));ui.label(format!("Energy spent: {} EU",status.energy_used));
                    ui.small(format!("Cube {}, {}, {}",status.focus[0],status.focus[1],status.focus[2]));
                });
                ui.separator();ui.label(&self.message);
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let available = ui.available_size();
            let aspect = 4. / 3.;
            let width = available.x.min(available.y * aspect).max(1.);
            let size = egui::vec2(width, width / aspect);
            ui.vertical_centered(|ui| {
                ui.add_space(((available.y - size.y) * 0.5).max(0.));
                let image =
                    egui::Image::new((self.world_image, size)).sense(egui::Sense::click_and_drag());
                let response = ui.add(image);
                self.world_rect = Some(response.rect);
                if response.clicked() {
                    response.request_focus();
                    if let Some(pos) = response.interact_pointer_pos() {
                        let ndc = [
                            (pos.x - response.rect.left()) / response.rect.width() * 2. - 1.,
                            1. - (pos.y - response.rect.top()) / response.rect.height() * 2.,
                        ];
                        match self.camera.ray(ndc).and_then(|ray| {
                            foundation_viewer::inspection::inspect(
                                self.session.as_ref().ok_or("session unavailable")?.world(),
                                ray,
                            )
                            .map_err(|e| e.to_string())
                        }) {
                            Ok(trace) => {
                                self.inspected = if let Outcome::Hit(hit) = trace.outcome {
                                    Some(hit)
                                } else {
                                    None
                                };
                                self.message = match trace.outcome {
                                    Outcome::Hit(_) => "Read-only block inspection.".into(),
                                    Outcome::Miss { .. } => {
                                        "No rendered block along this ray.".into()
                                    }
                                    other => format!("Inspection stopped: {other:?}"),
                                };
                            }
                            Err(error) => {
                                self.inspected = None;
                                self.message = error;
                            }
                        }
                    }
                }
                if let Some(hit) = self.inspected {
                    let base = self
                        .camera
                        .focus()
                        .cube()
                        .block_at(0)
                        .expect("validated camera cube");
                    if let (Some(x), Some(y), Some(z)) = (
                        hit.position.x.checked_sub(base.x),
                        hit.position.y.checked_sub(base.y),
                        hit.position.z.checked_sub(base.z),
                    ) && let Some(ndc) =
                        self.camera
                            .project_local([x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5])
                    {
                        let marker = egui::pos2(
                            response.rect.left() + (ndc[0] + 1.) * 0.5 * response.rect.width(),
                            response.rect.top() + (1. - ndc[1]) * 0.5 * response.rect.height(),
                        );
                        ui.painter().with_clip_rect(response.rect).circle_stroke(
                            marker,
                            7.,
                            egui::Stroke::new(2., egui::Color32::YELLOW),
                        );
                    }
                }
                if response.dragged_by(egui::PointerButton::Secondary) {
                    let delta = ui.ctx().input(|i| i.pointer.delta());
                    if let Err(error) = self.camera.orbit(delta.x * 0.006, delta.y * 0.006) {
                        self.message = error;
                    }
                }
                if response.hovered() {
                    let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.
                        && let Err(error) = self.camera.zoom(scroll * 0.02)
                    {
                        self.message = error;
                    }
                }
            });
        });
    }
}
