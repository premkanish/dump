// apps/terminal/src/ui/mod.rs
use common::*;
use common::security::{ApiCredentials, CredentialStore};
use egui::{Color32, RichText, Ui};
use std::collections::{HashMap, BTreeMap};

pub mod account_manager;
pub mod universe_settings;
pub mod asset_selector;
pub mod mode_control;
pub mod risk_panel;

pub use account_manager::AccountManagerState;
pub use universe_settings::UniverseSettingsState;
pub use asset_selector::AssetSelectorState;
pub use mode_control::ModeControlState;
pub use risk_panel::RiskPanelState;

// apps/terminal/src/ui/account_manager.rs
#[derive(Default)]
pub struct AccountManagerState {
    pub new_label: String,
    pub category: AssetCategory,
    pub venue: Venue,
    pub is_paper: bool,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
    pub accounts: BTreeMap<String, AccountInfo>,
    pub cred_store: Option<CredentialStore>,
    pub selected_account: Option<String>,
}

#[derive(Clone)]
pub struct AccountInfo {
    pub venue: Venue,
    pub is_paper: bool,
    pub balances: HashMap<String, f64>,
}

impl AccountManagerState {
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading("A1. Account Manager");
        
        // Initialize credential store
        if self.cred_store.is_none() {
            self.cred_store = Some(CredentialStore::new_simple());
        }
        
        ui.add_space(10.0);
        
        // Add new account section
        ui.group(|ui| {
            ui.label(RichText::new("Add New Account").strong());
            
            ui.horizontal(|ui| {
                ui.label("Label:");
                ui.text_edit_singleline(&mut self.new_label);
            });
            
            ui.horizontal(|ui| {
                ui.label("Category:");
                egui::ComboBox::from_id_source("category")
                    .selected_text(format!("{:?}", self.category))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.category, AssetCategory::Equity, "Equity");
                        ui.selectable_value(&mut self.category, AssetCategory::CryptoFutures, "Crypto Futures");
                    });
            });
            
            ui.horizontal(|ui| {
                ui.label("Exchange:");
                let venues = match self.category {
                    AssetCategory::Equity => vec![Venue::IBKR],
                    AssetCategory::CryptoFutures => vec![Venue::Hyperliquid, Venue::BinanceFutures],
                };
                
                egui::ComboBox::from_id_source("venue")
                    .selected_text(format!("{:?}", self.venue))
                    .show_ui(ui, |ui| {
                        for v in venues {
                            ui.selectable_value(&mut self.venue, v, format!("{:?}", v));
                        }
                    });
            });
            
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.is_paper, "Paper Trading");
            });
            
            ui.horizontal(|ui| {
                ui.label("API Key:");
                ui.text_edit_singleline(&mut self.api_key);
            });
            
            ui.horizontal(|ui| {
                ui.label("API Secret:");
                ui.add(egui::TextEdit::singleline(&mut self.api_secret).password(true));
            });
            
            if self.venue == Venue::BinanceFutures {
                ui.horizontal(|ui| {
                    ui.label("Passphrase:");
                    ui.add(egui::TextEdit::singleline(&mut self.passphrase).password(true));
                });
            }
            
            ui.horizontal(|ui| {
                if ui.button("💾 Save Account").clicked() {
                    self.save_account();
                }
                
                if ui.button("🗑 Clear").clicked() {
                    self.clear_form();
                }
            });
        });
        
        ui.add_space(20.0);
        
        // List configured accounts
        ui.group(|ui| {
            ui.label(RichText::new("Configured Accounts").strong());
            
            if self.accounts.is_empty() {
                ui.label(RichText::new("No accounts configured").italics().color(Color32::GRAY));
            } else {
                egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                    for (label, info) in &self.accounts {
                        ui.horizontal(|ui| {
                            let is_selected = self.selected_account.as_ref() == Some(label);
                            
                            if ui.selectable_label(is_selected, format!(
                                "{} - {:?} ({})",
                                label,
                                info.venue,
                                if info.is_paper { "Paper" } else { "Live" }
                            )).clicked() {
                                self.selected_account = Some(label.clone());
                            }
                            
                            if ui.button("💰 Check Balance").clicked() {
                                // TODO: Query balance from engine
                            }
                            
                            if ui.button("🗑").clicked() {
                                // Delete account
                            }
                        });
                        
                        // Show balances if available
                        if !info.balances.is_empty() {
                            ui.indent(label, |ui| {
                                for (asset, amount) in &info.balances {
                                    let color = if *amount < 10.0 {
                                        Color32::RED
                                    } else {
                                        Color32::GREEN
                                    };
                                    
                                    ui.colored_label(color, format!("{}: ${:.2}", asset, amount));
                                    
                                    if *amount < 10.0 {
                                        ui.label(RichText::new("⚠ No amount to trade").small().color(Color32::YELLOW));
                                    }
                                }
                            });
                        }
                    }
                });
            }
        });
    }
    
    fn save_account(&mut self) {
        if self.new_label.is_empty() || self.api_key.is_empty() || self.api_secret.is_empty() {
            tracing::warn!("Missing required fields");
            return;
        }
        
        let creds = if self.passphrase.is_empty() {
            ApiCredentials::new(self.api_key.clone(), self.api_secret.clone(), self.is_paper)
        } else {
            ApiCredentials::new(self.api_key.clone(), self.api_secret.clone(), self.is_paper)
                .with_passphrase(self.passphrase.clone())
        };
        
        if let Some(store) = &self.cred_store {
            match store.save(self.venue, &self.new_label, &creds) {
                Ok(_) => {
                    self.accounts.insert(
                        self.new_label.clone(),
                        AccountInfo {
                            venue: self.venue,
                            is_paper: self.is_paper,
                            balances: HashMap::new(),
                        },
                    );
                    
                    tracing::info!("Account {} saved successfully", self.new_label);
                    self.clear_form();
                }
                Err(e) => {
                    tracing::error!("Failed to save account: {}", e);
                }
            }
        }
    }
    
    fn clear_form(&mut self) {
        self.new_label.clear();
        self.api_key.clear();
        self.api_secret.clear();
        self.passphrase.clear();
    }
}

// apps/terminal/src/ui/universe_settings.rs
#[derive(Default)]
pub struct UniverseSettingsState {
    pub gecko_terminal: String,
    pub birdeye: String,
    pub the_graph: String,
    pub crypto_panic: String,
    pub flipside: String,
}

impl UniverseSettingsState {
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading("A2. Universe Creation Settings");
        ui.add_space(10.0);
        
        ui.group(|ui| {
            ui.label("Data Source API Keys");
            
            ui.horizontal(|ui| {
                ui.label("GeckoTerminal:");
                ui.text_edit_singleline(&mut self.gecko_terminal);
            });
            
            ui.horizontal(|ui| {
                ui.label("Birdeye:");
                ui.text_edit_singleline(&mut self.birdeye);
            });
            
            ui.horizontal(|ui| {
                ui.label("The Graph:");
                ui.text_edit_singleline(&mut self.the_graph);
            });
            
            ui.horizontal(|ui| {
                ui.label("CryptoPanic:");
                ui.text_edit_singleline(&mut self.crypto_panic);
            });
            
            ui.horizontal(|ui| {
                ui.label("Flipside:");
                ui.text_edit_singleline(&mut self.flipside);
            });
            
            if ui.button("💾 Save Keys").clicked() {
                tracing::info!("Data source keys saved");
            }
        });
    }
}

// apps/terminal/src/ui/asset_selector.rs
#[derive(Default)]
pub struct AssetSelectorState {
    pub venue: Venue,
    pub query: String,
    pub suggestions: Vec<String>,
    pub selected_assets: Vec<String>,
    pub auto_universe: bool,
}

impl AssetSelectorState {
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading("A3. Asset Selection");
        ui.add_space(10.0);
        
        ui.checkbox(&mut self.auto_universe, "Use Automatic Universe Selection");
        
        if !self.auto_universe {
            ui.group(|ui| {
                ui.label("Manual Asset Selection");
                
                ui.horizontal(|ui| {
                    ui.label("Venue:");
                    egui::ComboBox::from_id_source("asset_venue")
                        .selected_text(format!("{:?}", self.venue))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.venue, Venue::Hyperliquid, "Hyperliquid");
                            ui.selectable_value(&mut self.venue, Venue::BinanceFutures, "Binance Futures");
                            ui.selectable_value(&mut self.venue, Venue::IBKR, "IBKR");
                        });
                });
                
                ui.horizontal(|ui| {
                    ui.label("Search:");
                    if ui.text_edit_singleline(&mut self.query).changed() {
                        if self.query.len() >= 2 {
                            self.fetch_suggestions();
                        }
                    }
                });
                
                if !self.suggestions.is_empty() {
                    ui.label("Suggestions:");
                    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                        for suggestion in &self.suggestions.clone() {
                            if ui.button(suggestion).clicked() {
                                if !self.selected_assets.contains(suggestion) {
                                    self.selected_assets.push(suggestion.clone());
                                }
                            }
                        }
                    });
                }
            });
            
            ui.add_space(10.0);
            
            ui.group(|ui| {
                ui.label(RichText::new("Selected Assets").strong());
                
                if self.selected_assets.is_empty() {
                    ui.label(RichText::new("No assets selected").italics().color(Color32::GRAY));
                } else {
                    for asset in self.selected_assets.clone() {
                        ui.horizontal(|ui| {
                            ui.label(&asset);
                            if ui.button("✖").clicked() {
                                self.selected_assets.retain(|a| a != &asset);
                            }
                        });
                    }
                }
            });
        } else {
            ui.label(RichText::new("Engine will automatically select top assets based on universe scoring")
                .italics()
                .color(Color32::LIGHT_BLUE));
        }
    }
    
    fn fetch_suggestions(&mut self) {
        // TODO: Call engine API for autocomplete
        self.suggestions = vec![
            format!("{}USDT", self.query.to_uppercase()),
            format!("{}-PERP", self.query.to_uppercase()),
        ];
    }
}

// apps/terminal/src/ui/mode_control.rs
#[derive(Default)]
pub struct ModeControlState {
    pub mode: TradingMode,
}

impl ModeControlState {
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Mode:");
            
            let modes = [
                (TradingMode::Backtest, "📊 Backtest", Color32::BLUE),
                (TradingMode::Paper, "📝 Paper", Color32::YELLOW),
                (TradingMode::Live, "🔴 Live", Color32::RED),
                (TradingMode::Paused, "⏸ Paused", Color32::GRAY),
            ];
            
            for (mode, label, color) in modes {
                let is_selected = self.mode == mode;
                let button = egui::Button::new(RichText::new(label).color(color))
                    .selected(is_selected);
                
                if ui.add(button).clicked() {
                    self.mode = mode;
                    tracing::info!("Mode changed to {:?}", mode);
                    // TODO: Send mode change to engine
                }
            }
        });
    }
}

// apps/terminal/src/ui/risk_panel.rs
#[derive(Default)]
pub struct RiskPanelState {
    pub risk_snapshot: RiskSnapshot,
    pub perf_metrics: PerformanceMetrics,
}

impl RiskPanelState {
    pub fn update_from_ws(&mut self, client: &crate::ws_client::MetricsClient) {
        // TODO: Get latest data from WebSocket client
    }
    
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.columns(3, |cols| {
            // Column 1: Risk Metrics
            cols[0].group(|ui| {
                ui.heading("Risk");
                ui.add_space(5.0);
                
                self.metric_row(ui, "Gross Notional:", format!("${:.0}", self.risk_snapshot.gross_notional));
                self.metric_row(ui, "Net Notional:", format!("${:.0}", self.risk_snapshot.net_notional));
                self.metric_row(ui, "Positions:", format!("{}", self.risk_snapshot.num_positions));
                self.metric_row(ui, "Margin Used:", format!("${:.0}", self.risk_snapshot.total_margin_used));
                self.metric_row(ui, "Available:", format!("${:.0}", self.risk_snapshot.available_margin));
                
                ui.add_space(5.0);
                
                let kill_color = if self.risk_snapshot.kill_switch_active {
                    Color32::RED
                } else {
                    Color32::GREEN
                };
                
                ui.colored_label(kill_color, if self.risk_snapshot.kill_switch_active {
                    "🛑 KILL SWITCH ACTIVE"
                } else {
                    "✓ System Normal"
                });
            });
            
            // Column 2: PnL
            cols[1].group(|ui| {
                ui.heading("P&L");
                ui.add_space(5.0);
                
                let unrealized_color = if self.risk_snapshot.unrealized_pnl >= 0.0 {
                    Color32::GREEN
                } else {
                    Color32::RED
                };
                
                let realized_color = if self.risk_snapshot.realized_pnl >= 0.0 {
                    Color32::GREEN
                } else {
                    Color32::RED
                };
                
                ui.horizontal(|ui| {
                    ui.label("Unrealized:");
                    ui.colored_label(unrealized_color, format!("${:.2}", self.risk_snapshot.unrealized_pnl));
                });
                
                ui.horizontal(|ui| {
                    ui.label("Realized:");
                    ui.colored_label(realized_color, format!("${:.2}", self.risk_snapshot.realized_pnl));
                });
                
                ui.horizontal(|ui| {
                    ui.label("Total:");
                    let total_color = if self.risk_snapshot.total_pnl >= 0.0 {
                        Color32::GREEN
                    } else {
                        Color32::RED
                    };
                    ui.colored_label(total_color, format!("${:.2}", self.risk_snapshot.total_pnl));
                });
                
                ui.horizontal(|ui| {
                    ui.label("Daily:");
                    let daily_color = if self.risk_snapshot.daily_pnl >= 0.0 {
                        Color32::GREEN
                    } else {
                        Color32::RED
                    };
                    ui.colored_label(daily_color, format!("${:.2}", self.risk_snapshot.daily_pnl));
                });
            });
            
            // Column 3: Performance
            cols[2].group(|ui| {
                ui.heading("Performance");
                ui.add_space(5.0);
                
                ui.label(RichText::new("Latency (μs)").strong());
                self.metric_row(ui, "Ingest p99:", format!("{:.0}", self.perf_metrics.ingest_p99_us));
                self.metric_row(ui, "Feature p99:", format!("{:.0}", self.perf_metrics.feature_p99_us));
                self.metric_row(ui, "Model p99:", format!("{:.0}", self.perf_metrics.model_p99_us));
                self.metric_row(ui, "Route p99:", format!("{:.0}", self.perf_metrics.route_p99_us));
                
                ui.add_space(5.0);
                
                self.metric_row(ui, "Snapshots/s:", format!("{:.1}", self.perf_metrics.snapshots_per_sec));
                self.metric_row(ui, "Dropped Frames:", format!("{}", self.perf_metrics.dropped_frames));
                self.metric_row(ui, "Model Timeouts:", format!("{}", self.perf_metrics.model_timeouts));
            });
        });
    }
    
    fn metric_row(&self, ui: &mut Ui, label: &str, value: String) {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.monospace(&value);
            });
        });
    }
}