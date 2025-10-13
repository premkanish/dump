// apps/terminal/src/main.rs
#![windows_subsystem = "windows"]

mod ui;
mod ws_client;

use common::*;
use eframe::egui;
use ui::*;
use ws_client::MetricsClient;

fn main() -> Result<(), eframe::Error> {
    // Setup logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1920.0, 1080.0])
            .with_title("HFT Trading Terminal")
            .with_resizable(true),
        ..Default::default()
    };
    
    eframe::run_native(
        "HFT Terminal",
        options,
        Box::new(|cc| Box::new(TerminalApp::new(cc))),
    )
}

struct TerminalApp {
    // UI state
    account_manager: AccountManagerState,
    universe_settings: UniverseSettingsState,
    asset_selector: AssetSelectorState,
    mode_control: ModeControlState,
    risk_panel: RiskPanelState,
    
    // WebSocket client
    ws_client: Option<MetricsClient>,
    
    // Runtime
    runtime: tokio::runtime::Runtime,
}

impl TerminalApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Configure fonts
        configure_fonts(&cc.egui_ctx);
        
        // Set dark theme
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        
        Self {
            account_manager: AccountManagerState::default(),
            universe_settings: UniverseSettingsState::default(),
            asset_selector: AssetSelectorState::default(),
            mode_control: ModeControlState::default(),
            risk_panel: RiskPanelState::default(),
            ws_client: None,
            runtime,
        }
    }
    
    fn connect_to_engine(&mut self, url: &str) {
        let (tx, rx) = std::sync::mpsc::channel();
        let url_owned = url.to_string();
        
        self.runtime.spawn(async move {
            match MetricsClient::connect(&url_owned).await {
                Ok(client) => {
                    let _ = tx.send(Ok(client));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });
        
        // Try to receive immediately (non-blocking)
        if let Ok(result) = rx.try_recv() {
            match result {
                Ok(client) => {
                    self.ws_client = Some(client);
                    tracing::info!("Connected to engine");
                }
                Err(e) => {
                    tracing::error!("Failed to connect: {}", e);
                }
            }
        }
    }
}

impl eframe::App for TerminalApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request continuous repaint for 60fps
        ctx.request_repaint();
        
        // Top menu bar
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                
                ui.menu_button("Settings", |ui| {
                    if ui.button("Preferences").clicked() {
                        // Open preferences
                    }
                });
                
                ui.separator();
                
                // Connection status
                let connected = self.ws_client.is_some();
                let color = if connected {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(color, if connected { "● Connected" } else { "● Disconnected" });
                
                if !connected {
                    if ui.button("Connect").clicked() {
                        self.connect_to_engine("ws://localhost:8081/metrics");
                    }
                }
            });
        });
        
        // Main content area with tabs
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("High-Frequency Trading Terminal");
                ui.add_space(10.0);
                
                // Tabs
                egui::containers::CollapsingHeader::new("Account Management")
                    .default_open(true)
                    .show(ui, |ui| {
                        self.account_manager.ui(ui);
                    });
                
                egui::containers::CollapsingHeader::new("Universe Settings")
                    .default_open(false)
                    .show(ui, |ui| {
                        self.universe_settings.ui(ui);
                    });
                
                egui::containers::CollapsingHeader::new("Asset Selection")
                    .default_open(false)
                    .show(ui, |ui| {
                        self.asset_selector.ui(ui);
                    });
                
                ui.add_space(20.0);
                ui.separator();
                ui.add_space(10.0);
                
                // Mode control
                ui.heading("Trading Control");
                self.mode_control.ui(ui);
                
                ui.add_space(20.0);
                ui.separator();
                ui.add_space(10.0);
                
                // Risk panel
                ui.heading("Risk & Performance");
                if let Some(ws_client) = &self.ws_client {
                    self.risk_panel.update_from_ws(ws_client);
                }
                self.risk_panel.ui(ui);
            });
        });
        
        // Status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("FPS: {:.1}", ctx.input(|i| i.stable_dt.recip())));
                ui.separator();
                ui.label(format!("Time: {}", chrono::Local::now().format("%H:%M:%S")));
            });
        });
    }
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    
    // Add monospace font for numbers
    fonts.families.insert(
        egui::FontFamily::Monospace,
        vec!["Hack".to_owned(), "monospace".to_owned()],
    );
    
    ctx.set_fonts(fonts);
}