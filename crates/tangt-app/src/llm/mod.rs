//! The in-app plan assistant: describe a change in prose, the model calls the
//! same tools MCP exposes, and every edit lands as an undoable `Op`.
//!
//! The model runs on a worker thread and never sees app state — it asks for
//! tool calls, and the UI thread executes them against the document, exactly
//! like the MCP bridge does.

pub mod agent;
pub mod model;

use eframe::egui;
use model::{
    Api, ChatMessage, GenerateRequest, ModelConfig, ModelError, ToolCall, ToolDef,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Grounded and conservative: the assistant edits a real plan.
const TEMPERATURE: f32 = 0.2;

pub fn system_prompt(chart_summary: &str) -> String {
    format!(
        "You edit a Gantt chart through tools. The chart is a plain-text file; every tool call \
         rewrites it and is undoable by the user.\n\n\
         Rules:\n\
         - Call list_tasks before changing anything you did not just create, so you use real ids.\n\
         - Dates are always YYYY-MM-DD. Durations are whole working days.\n\
         - Prefer add_dependency over hard-coded start dates, so the plan reflows when work slips.\n\
         - Make only the changes the user asked for. Do not invent tasks to be helpful.\n\
         - When you are done, reply in one or two plain sentences saying what you changed. \
           If you changed nothing, say why.\n\n\
         Current chart:\n{chart_summary}"
    )
}

/// What the worker is doing, polled by the UI each frame.
#[derive(Default)]
pub struct AssistStatus {
    pub running: bool,
    /// Streaming reply so far.
    pub streaming: String,
    /// Tool calls made this run, for the activity list.
    pub activity: Vec<String>,
    pub outcome: Option<Result<String, String>>,
}

/// A request from the worker for the UI thread to run a tool.
pub struct ToolRequest {
    pub call: ToolCall,
    pub reply: std::sync::mpsc::Sender<Result<String, String>>,
}

pub struct AssistJob {
    pub prompt: String,
    pub chart_summary: String,
    pub config: ModelConfig,
    pub tools: Vec<ToolDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssistPrefs {
    pub config: ModelConfig,
    /// Panel visibility, persisted like the other panes.
    pub show_panel: bool,
}

pub struct AssistEngine {
    tx: Sender<AssistJob>,
    pub tool_rx: Receiver<ToolRequest>,
    status: Arc<Mutex<AssistStatus>>,
    cancel: Arc<AtomicBool>,
    pub prefs: AssistPrefs,
    /// Draft text in the prompt box.
    pub draft: String,
    /// Model names discovered from the endpoint, for the settings dropdown.
    pub available: Vec<String>,
    pub probe: Option<String>,
}

impl AssistEngine {
    pub fn spawn(ctx: &egui::Context, prefs: AssistPrefs) -> Self {
        let (tx, job_rx) = std::sync::mpsc::channel::<AssistJob>();
        let (tool_tx, tool_rx) = std::sync::mpsc::channel::<ToolRequest>();
        let status = Arc::new(Mutex::new(AssistStatus::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let (status, cancel, ctx) = (status.clone(), cancel.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("tangt-assist".into())
                .spawn(move || worker(job_rx, tool_tx, status, cancel, ctx))
                .expect("spawning the assistant thread");
        }
        AssistEngine {
            tx,
            tool_rx,
            status,
            cancel,
            prefs,
            draft: String::new(),
            available: Vec::new(),
            probe: None,
        }
    }

    pub fn ask(&self, job: AssistJob) {
        self.cancel.store(false, Ordering::Relaxed);
        {
            let mut status = self.status.lock().expect("status lock");
            *status = AssistStatus { running: true, ..Default::default() };
        }
        let _ = self.tx.send(job);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn with_status<T>(&self, f: impl FnOnce(&AssistStatus) -> T) -> T {
        f(&self.status.lock().expect("status lock"))
    }

    pub fn take_outcome(&self) -> Option<Result<String, String>> {
        self.status.lock().expect("status lock").outcome.take()
    }
}

fn worker(
    jobs: Receiver<AssistJob>,
    tool_tx: Sender<ToolRequest>,
    status: Arc<Mutex<AssistStatus>>,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    while let Ok(job) = jobs.recv() {
        let backend = job.config.build();
        let request = GenerateRequest {
            model: job.config.model.clone(),
            system: Some(system_prompt(&job.chart_summary)),
            history: vec![ChatMessage::user(job.prompt)],
            tools: job.tools,
            temperature: Some(TEMPERATURE),
            max_tokens: None,
        };

        let mut execute = |call: &ToolCall| -> Result<String, String> {
            {
                let mut s = status.lock().expect("status lock");
                s.activity.push(call.name.clone());
            }
            ctx.request_repaint();
            let (reply, rx) = std::sync::mpsc::channel();
            tool_tx
                .send(ToolRequest { call: call.clone(), reply })
                .map_err(|_| "tangt closed".to_string())?;
            ctx.request_repaint();
            rx.recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|_| "tangt did not answer the tool call".to_string())?
        };

        let mut on_token = |text: &str| {
            {
                let mut s = status.lock().expect("status lock");
                s.streaming.push_str(text);
            }
            ctx.request_repaint();
            if cancel.load(Ordering::Relaxed) {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        };

        let result = agent::run_agent(
            backend.as_ref(),
            request,
            &mut execute,
            agent::MAX_ITERATIONS,
            &mut on_token,
            &cancel,
        );

        let mut s = status.lock().expect("status lock");
        s.running = false;
        s.outcome = Some(match result {
            Ok(outcome) if outcome.text.trim().is_empty() => {
                Ok("(the assistant finished without a summary)".to_string())
            }
            Ok(outcome) => Ok(outcome.text),
            Err(ModelError::Unreachable(m)) | Err(ModelError::Read(m)) => Err(m),
        });
        drop(s);
        ctx.request_repaint();
    }
}

impl AssistEngine {
    /// Settings block rendered inside the Preferences window.
    pub fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Describe a change in plain language and the assistant edits the chart with the \
             same tools MCP exposes. Runs against a local model you choose — tangt never \
             sends your plan anywhere else.",
        );
        ui.add_space(6.0);
        egui::ComboBox::from_label("Endpoint")
            .selected_text(self.prefs.config.api.label())
            .show_ui(ui, |ui| {
                for api in [Api::Ollama, Api::OpenAiCompatible] {
                    if ui
                        .selectable_label(self.prefs.config.api == api, api.label())
                        .clicked()
                    {
                        self.prefs.config.api = api;
                        self.prefs.config.base_url = api.default_url().to_string();
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label("URL");
            ui.add(
                egui::TextEdit::singleline(&mut self.prefs.config.base_url)
                    .desired_width(260.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Model");
            ui.add(
                egui::TextEdit::singleline(&mut self.prefs.config.model).desired_width(260.0),
            );
            if ui.button("Find models").clicked() {
                match self.prefs.config.build().list_models() {
                    Ok(models) if models.is_empty() => {
                        self.probe = Some("connected, but no models are installed".into());
                        self.available.clear();
                    }
                    Ok(models) => {
                        self.probe = Some(format!("{} model(s) available", models.len()));
                        self.available = models;
                    }
                    Err(e) => {
                        self.probe = Some(e.to_string());
                        self.available.clear();
                    }
                }
            }
        });
        if !self.available.is_empty() {
            egui::ComboBox::from_label("Installed")
                .selected_text(&self.prefs.config.model)
                .show_ui(ui, |ui| {
                    for name in self.available.clone() {
                        if ui
                            .selectable_label(self.prefs.config.model == name, &name)
                            .clicked()
                        {
                            self.prefs.config.model = name;
                        }
                    }
                });
        }
        if let Some(probe) = &self.probe {
            ui.weak(probe);
        }
    }
}
