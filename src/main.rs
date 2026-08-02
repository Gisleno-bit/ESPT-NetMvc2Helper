// v0.3 - deteccion estricta por firma de netcode 60 Hz.
// La consola se deja a proposito: cualquier error sale ahi.

mod capture;

use capture::{Compartido, Snapshot};
use eframe::egui;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> Result<(), eframe::Error> {
    println!("[netmon] arrancando...");

    let estado: Compartido = Arc::new(Mutex::new(Snapshot::default()));

    {
        let e = estado.clone();
        std::thread::spawn(move || capture::hilo_captura(e));
    }
    {
        let e = estado.clone();
        std::thread::spawn(move || capture::hilo_ping(e));
    }

    let opciones = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 540.0])
            .with_decorations(true)
            .with_transparent(false)
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "MvC NetMon",
        opciones,
        Box::new(|_cc| {
            Box::new(App {
                estado,
                overlay: false,
            })
        }),
    )
}

struct App {
    estado: Compartido,
    overlay: bool,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(150));

        let s = self.estado.lock().unwrap().clone();

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("MvC NetMon");
                let (txt, col) = if s.error.is_some() {
                    ("ERROR", egui::Color32::from_rgb(255, 90, 90))
                } else if !s.activo {
                    ("iniciando", egui::Color32::GRAY)
                } else if s.peer.is_some() {
                    ("EN PARTIDA", egui::Color32::from_rgb(90, 220, 110))
                } else {
                    ("esperando", egui::Color32::from_rgb(220, 190, 90))
                };
                ui.label(egui::RichText::new(txt).color(col).strong());
            });

            ui.separator();

            if let Some(e) = &s.error {
                ui.colored_label(egui::Color32::from_rgb(255, 120, 120), e);
                ui.add_space(6.0);
                ui.label("Cierra y abre como administrador.");
                return;
            }

            match &s.peer {
                None => {
                    ui.add_space(8.0);
                    ui.label("Sin partida detectada.");
                    ui.weak("Busco un flujo simetrico a ~60 Hz.");
                    ui.add_space(8.0);
                }
                Some(p) => {
                    ui.label(format!("Rival: {}", p.ip));
                    ui.add_space(4.0);

                    let (cj, etiqueta) = calidad_jitter(p.jitter_ms);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.1}", p.jitter_ms))
                                .color(cj)
                                .strong()
                                .size(34.0),
                        );
                        ui.vertical(|ui| {
                            ui.add_space(10.0);
                            ui.label("ms jitter");
                            ui.colored_label(cj, etiqueta);
                        });
                    });

                    grafico(ui, &p.hist, p.mean_ms);
                    ui.add_space(4.0);

                    egui::Grid::new("stats").num_columns(2).show(ui, |ui| {
                        ui.label("ping ICMP");
                        match p.rtt_ms {
                            Some(v) => ui.label(format!("{} ms", v)),
                            None => ui.label("sin respuesta"),
                        };
                        ui.end_row();

                        ui.label("intervalo");
                        ui.label(format!("{:.1} ms", p.mean_ms));
                        ui.end_row();

                        ui.label("paquetes");
                        ui.label(format!("{:.0} in / {:.0} out", p.pps_in, p.pps_out));
                        ui.end_row();

                        ui.label("pico");
                        ui.label(format!("{:.0} ms", p.max_gap_ms));
                        ui.end_row();

                        ui.label("huecos");
                        ui.label(format!("{}", p.huecos));
                        ui.end_row();
                    });
                }
            }

            ui.separator();

            ui.collapsing("Diagnostico", |ui| {
                egui::Grid::new("diag").num_columns(2).show(ui, |ui| {
                    ui.label("IP local");
                    ui.label(&s.ip_local);
                    ui.end_row();

                    ui.label("paquetes IP");
                    ui.label(format!("{}", s.total_ip));
                    ui.end_row();

                    ui.label("de ellos UDP");
                    ui.label(format!("{}", s.total_udp));
                    ui.end_row();

                    ui.label("Steam/Valve");
                    ui.label(format!("{:.0} pps", s.valve_pps));
                    ui.end_row();

                    ui.label("IPs publicas");
                    ui.label(format!("{}", s.otros));
                    ui.end_row();
                });

                ui.add_space(8.0);
                ui.label(egui::RichText::new("Flujos vistos").strong());
                ui.weak("verde = cumple la firma de un juego");
                ui.add_space(4.0);

                if s.candidatos.is_empty() {
                    ui.weak("ninguno todavia");
                } else {
                    egui::Grid::new("cands").num_columns(4).striped(true).show(ui, |ui| {
                        ui.label(egui::RichText::new("IP").weak());
                        ui.label(egui::RichText::new("in").weak());
                        ui.label(egui::RichText::new("out").weak());
                        ui.label(egui::RichText::new("interv.").weak());
                        ui.end_row();

                        for c in &s.candidatos {
                            let col = if c.es_juego {
                                egui::Color32::from_rgb(90, 220, 110)
                            } else {
                                egui::Color32::GRAY
                            };
                            ui.colored_label(col, &c.ip);
                            ui.colored_label(col, format!("{:.0}", c.pps_in));
                            ui.colored_label(col, format!("{:.0}", c.pps_out));
                            ui.colored_label(col, format!("{:.0} ms", c.mean_ms));
                            ui.end_row();
                        }
                    });
                }
            });

            ui.separator();

            let mut grabando = s.grabando;
            if ui.checkbox(&mut grabando, "Grabar log CSV").changed() {
                self.estado.lock().unwrap().grabando = grabando;
            }

            let mut laxo = s.laxo;
            if ui.checkbox(&mut laxo, "Modo laxo (acepta cualquier flujo)").changed() {
                self.estado.lock().unwrap().laxo = laxo;
            }
            ui.weak("Solo para depurar. Engancha Discord y demas.");

            if ui.checkbox(&mut self.overlay, "Modo overlay").changed() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!self.overlay));
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if self.overlay {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                }));
            }
            });
        });
    }
}

fn calidad_jitter(j: f64) -> (egui::Color32, &'static str) {
    if j <= 0.0 {
        (egui::Color32::GRAY, "midiendo")
    } else if j < 3.0 {
        (egui::Color32::from_rgb(90, 220, 110), "excelente")
    } else if j < 7.0 {
        (egui::Color32::from_rgb(170, 215, 110), "bien")
    } else if j < 15.0 {
        (egui::Color32::from_rgb(230, 200, 100), "regular")
    } else {
        (egui::Color32::from_rgb(235, 110, 100), "malo")
    }
}

fn grafico(ui: &mut egui::Ui, hist: &[f32], media: f64) {
    let (resp, pintor) =
        ui.allocate_painter(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
    let r = resp.rect;

    pintor.rect_filled(
        r,
        3.0,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 14),
    );

    if hist.is_empty() || media <= 0.0 {
        return;
    }

    let techo = ((media * 2.0) as f32).max(30.0);
    let ancho = r.width() / hist.len() as f32;

    let y_media = r.bottom() - (media as f32 / techo).clamp(0.0, 1.0) * r.height();
    pintor.line_segment(
        [egui::pos2(r.left(), y_media), egui::pos2(r.right(), y_media)],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 200, 255)),
    );

    for (i, v) in hist.iter().enumerate() {
        let frac = (v / techo).clamp(0.0, 1.0);
        let h = frac * r.height();
        let x = r.left() + i as f32 * ancho;
        let desviacion = (*v as f64 - media).abs();
        let col = if desviacion < media * 0.25 {
            egui::Color32::from_rgb(90, 190, 110)
        } else if desviacion < media * 0.75 {
            egui::Color32::from_rgb(220, 200, 100)
        } else {
            egui::Color32::from_rgb(235, 110, 100)
        };
        pintor.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, r.bottom() - h),
                egui::pos2(x + ancho * 0.8, r.bottom()),
            ),
            0.0,
            col,
        );
    }
}
