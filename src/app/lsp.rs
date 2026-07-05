use std::collections::HashMap;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use eyre::{Context, Result};
use module::Merge;
use serde::Deserialize;
use tokio::sync::{Mutex, MutexGuard};
use tower_lsp::{LanguageServer, LspService, Server, lsp_types::*};

use crate::discord::*;
use crate::util::SystemTimeExt;

const CLIENT_ID: &str = "1523025249845903410";

///////////////////////////////////////////////////////////////////////////////

#[derive(Debug, clap::Parser)]
#[command(name = "lsp")]
pub struct Command {}

///////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Default, Deserialize, Merge)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct File {}

///////////////////////////////////////////////////////////////////////////////

pub async fn run() -> Result<ExitCode> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|_| LspTask {
        state: Mutex::new(State {
            client: None,
            start: SystemTime::now(),
            documents: HashMap::new(),

            last_update: None,
            discord: Discord::builder().client_id(CLIENT_ID).finish(), /* TODO: fetch client id from config file */
        }),
    });

    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(ExitCode::SUCCESS)
}

struct LspTask {
    state: Mutex<State>,
}

struct State {
    client: Option<String>,
    start: SystemTime,
    documents: HashMap<Url, Document>,

    last_update: Option<Instant>,
    discord: Discord,
}

struct Document {
    language: String,
}

impl LspTask {
    fn build_activity(
        &self,
        state: &MutexGuard<'_, State>,
        active_document: &Url,
    ) -> Activity<'static> {
        let State { ref start, .. } = **state;

        let mut activity = Activity::new()
            .name("todo")
            .activity_type(ActivityType::Playing)
            .status_display_type(StatusDisplayType::Name)
            .timestamps(Timestamps::new().start(start.duration_since_epoch().as_secs() as i64))
            .party(Party::new().size([1, 1]));

        activity = activity.details("details").state("state");
        activity
    }

    #[instrument(skip(self, state))]
    async fn update_presence(&self, state: &mut MutexGuard<'_, State>, active_document: &Url) {
        let now = Instant::now();

        if state.last_update.is_some_and(|x| now - x < Duration::from_secs(1) /* TODO: fetch interval from config file */) {
            debug!("skip update");
            return;
        }
        

        debug!("update");
        state.last_update = Some(now);
        let activity = self.build_activity(state, active_document);

        if let Err(e) = state
            .discord
            .set_activity(activity)
            .await
            .context("cannot update rich presence")
        {
            error!("{e:#}");
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for LspTask {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        debug!("initialize(params={params:#?})");

        let mut state = self.state.lock().await;

        if let Some(client_info) = params.client_info {
            state.client = Some(client_info.name);
        }

        state.start = SystemTime::now();

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),

                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, params: InitializedParams) {
        debug!("initialized(params={params:#?})");
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        debug!("shutdown");
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        debug!("did_open(params={params:#?})");

        let mut state = self.state.lock().await;

        state
            .documents
            .entry(params.text_document.uri)
            .insert_entry(Document {
                language: params.text_document.language_id,
            });
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp::jsonrpc::Result<Option<Hover>> {
        debug!("hover(params={params:#?})");

        let mut state = self.state.lock().await;
        self.update_presence(
            &mut state,
            &params.text_document_position_params.text_document.uri,
        )
        .await;

        Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String("hovering file".to_string())),
            range: None,
        }))
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        debug!("did_change(params={params:#?})");

        let mut state = self.state.lock().await;
        self.update_presence(&mut state, &params.text_document.uri)
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        debug!("did_save(params={params:#?})");

        let mut state = self.state.lock().await;
        self.update_presence(&mut state, &params.text_document.uri)
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        debug!("did_close(params={params:#?})");

        let mut state = self.state.lock().await;
        state.documents.remove(&params.text_document.uri);
    }
}
