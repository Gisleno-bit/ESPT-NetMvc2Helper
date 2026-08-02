// Oculta la consola en release, deja la ventana negra en debug.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;

use capture::{Compartido, Snapshot};
use eframe::egui;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> Result<(), eframe::Error> {
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
            .with_inner_size([300.0, 250.0])
            .with_position([24.0, 24.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        "MvC NetMon",
        opciones,
        Box::new(|_cc| Box::new(App { estado })),
    )
}

struct App {
    estado: Compartido,
}

impl eframe::App for App {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(120));

        let s = self.estado.lock().unwrap().clone();

        let marco = egui::Frame::none()
            .fill(egui::Color32::from_rgba_unmultiplied(8, 10, 14, 205))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(11.0))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(90, 100, 120, 130),
            ));

        egui::CentralPanel::default().frame(marco).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("MvC NetMon")
                        .color(egui::Color32::from_rgb(120, 200, 255))
                        .strong()
                        .size(13.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (txt, col) = if s.error.is_some() {
                        ("ERROR", egui::Color32::from_rgb(255, 90, 90))
                    } else if !s.activo {
                        ("iniciando", egui::Color32::GRAY)
                    } else if s.peer.is_some() {
                        ("en partida", egui::Color32::from_rgb(120, 230, 130))
                    } else {
                        ("esperando", egui::Color32::from_rgb(220, 190, 90))
                    };
                    ui.label(egui::RichText::new(txt).color(col).size(11.0));
                });
            });

            ui.separator();

            if let Some(e) = &s.error {
                ui.label(
                    egui::RichText::new(e)
                        .color(egui::Color32::from_rgb(255, 120, 120))
                        .size(11.0),
                );
                return;
            }

            match &s.peer {
                None => {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Sin partida activa.")
                            .color(egui::Color32::GRAY)
                            .size(11.0),
                    );
                    ui.label(
                        egui::RichText::new("Entra en un combate online.")
                            .color(egui::Color32::DARK_GRAY)
                            .size(10.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("Steam/Valve: {:.0} pps", s.valve_pps))
                            .color(egui::Color32::DARK_GRAY)
                            .size(10.0),
                    );
                }
                Some(p) => {
                    ui.label(
                        egui::RichText::new(format!("rival  {}", p.ip))
                            .color(egui::Color32::from_rgb(200, 210, 225))
                            .size(11.0),
                    );

                    ui.add_space(3.0);

                    // JITTER: la metrica principal
                    let (cj, etiqueta) = calidad_jitter(p.jitter_ms);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.1}", p.jitter_ms))
                                .color(cj)
                                .strong()
                                .size(30.0),
                        );
                        ui.vertical(|ui| {
                            ui.add_space(9.0);
                            ui.label(
                                egui::RichText::new("ms jitter")
                                    .color(egui::Color32::GRAY)
                                    .size(11.0),
                            );
                            ui.label(egui::RichText::new(etiqueta).color(cj).size(11.0));
                        });
                    });

                    grafico(ui, &p.hist, p.mean_ms);

                    ui.add_space(3.0);

                    fila(
                        ui,
                        "ping ICMP",
                        match p.rtt_ms {
                            Some(v) => format!("{} ms", v),
                            None => "sin respuesta".into(),
                        },
                        match p.rtt_ms {
                            Some(v) if v < 40 => egui::Color32::from_rgb(120, 230, 130),
                            Some(v) if v < 90 => egui::Color32::from_rgb(230, 210, 110),
                            Some(_) => egui::Color32::from_rgb(240, 130, 110),
                            None => egui::Color32::DARK_GRAY,
                        },
                    );
                    fila(
                        ui,
                        "intervalo",
                        format!("{:.1} ms", p.mean_ms),
                        egui::Color32::from_rgb(190, 200, 215),
                    );
                    fila(
                        ui,
                        "paquetes",
                        format!("{:.0} in / {:.0} out", p.pps_in, p.pps_out),
                        egui::Color32::from_rgb(190, 200, 215),
                    );
                    fila(
                        ui,
                        "pico",
                        format!("{:.0} ms", p.max_gap_ms),
                        if p.max_gap_ms > 100.0 {
                            egui::Color32::from_rgb(240, 130, 110)
                        } else {
                            egui::Color32::from_rgb(190, 200, 215)
                        },
                    );
                    fila(
                        ui,
                        "huecos",
                        format!("{}", p.huecos),
                        if p.huecos > 0 {
                            egui::Color32::from_rgb(240, 130, 110)
                        } else {
                            egui::Color32::from_rgb(120, 230, 130)
                        },
                    );
                }
            }
        });
    }
}

fn fila(ui: &mut egui::Ui, nombre: &str, valor: String, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(nombre)
                .color(egui::Color32::from_rgb(110, 120, 135))
                .size(11.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(valor).color(color).size(11.0));
        });
    });
}

fn calidad_jitter(j: f64) -> (egui::Color32, &'static str) {
    if j <= 0.0 {
        (egui::Color32::GRAY, "midiendo")
    } else if j < 3.0 {
        (egui::Color32::from_rgb(120, 230, 130), "excelente")
    } else if j < 7.0 {
        (egui::Color32::from_rgb(180, 220, 120), "bien")
    } else if j < 15.0 {
        (egui::Color32::from_rgb(235, 205, 110), "regular")
    } else {
        (egui::Color32::from_rgb(240, 120, 110), "malo")
    }
}

/// Grafico de barras de los ultimos intervalos entre paquetes.
fn grafico(ui: &mut egui::Ui, hist: &[f32], media: f64) {
    let alto = 34.0;
    let (resp, pintor) =
        ui.allocate_painter(egui::vec2(ui.available_width(), alto), egui::Sense::hover());
    let r = resp.rect;

    pintor.rect_filled(
        r,
        3.0,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10),
    );

    if hist.is_empty() {
        return;
    }

    // Escala: el doble de la media, con minimo de 30 ms.
    let techo = ((media * 2.0) as f32).max(30.0);
    let ancho = r.width() / hist.len() as f32;

    // Linea de la media
    let y_media = r.bottom() - (media as f32 / techo).clamp(0.0, 1.0) * r.height();
    pintor.line_segment(
        [
            egui::pos2(r.left(), y_media),
            egui::pos2(r.right(), y_media),
        ],
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 200, 255, 70),
        ),
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
