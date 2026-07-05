use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use eyre::{Context, Result};
use module::Merge;
use serde::Deserialize;
use tokio::sync::{Mutex, MutexGuard};
use tower_lsp::{LanguageServer, LspService, Server, lsp_types::*};

use crate::discord::*;
use crate::util::{SystemTimeExt, find_repo_root, get_vcs_branch, home_dir};

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
    #[instrument(skip(self, state))]
    async fn update_presence(
        &self,
        state: &mut MutexGuard<'_, State>,
        active_document: Option<&Url>,
    ) {
        let now = Instant::now();

        if state.last_update.is_some_and(
            |x| now - x < Duration::from_secs(1), /* TODO: fetch interval from config file */
        ) {
            debug!("skip update");
            return;
        }

        debug!("update");
        state.last_update = Some(now);

        let State { ref start, .. } = **state;

        let mut activity = Activity::new()
            .activity_type(ActivityType::Playing)
            .status_display_type(StatusDisplayType::Name)
            .timestamps(Timestamps::new().start(start.duration_since_epoch().as_secs() as i64))
            .party(Party::new().size([1, 1]));

        if let Some(ref client) = state.client {
            activity = activity.name(client.clone());
        }

        if let Some(active_document) = active_document {
            let document_path = Path::new(active_document.path());

            activity = activity.details(
                None.or_else(|| {
                    let repo = find_repo_root(document_path)?;
                    let repo_name = repo.file_name()?;
                    let relative_document_path =
                        document_path.strip_prefix(repo).unwrap_or(document_path);

                    Some(format!(
                        "{}: {}",
                        repo_name.display(),
                        relative_document_path.display()
                    ))
                })
                .or_else(|| {
                    let home = home_dir()?;

                    Some(match document_path.strip_prefix(home) {
                        Ok(x) => format!("{}", Path::new("~").join(x).display()),
                        Err(_) => format!("{}", document_path.display()),
                    })
                })
                .unwrap_or_else(|| format!("{}", document_path.display())),
            );

            if let Some(branch) = get_vcs_branch(document_path.parent().unwrap_or(document_path))
                .await
                .ok()
                .flatten()
            {
                activity = activity.state(branch);
            }
        }

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
        let uri = params.text_document.uri;

        let mut state = self.state.lock().await;

        state.documents.entry(uri.clone()).insert_entry(Document {
            language: params.text_document.language_id,
        });

        self.update_presence(&mut state, Some(&uri)).await;
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp::jsonrpc::Result<Option<Hover>> {
        debug!("hover(params={params:#?})");

        let mut state = self.state.lock().await;
        self.update_presence(
            &mut state,
            Some(&params.text_document_position_params.text_document.uri),
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
        self.update_presence(&mut state, Some(&params.text_document.uri))
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        debug!("did_save(params={params:#?})");

        let mut state = self.state.lock().await;
        self.update_presence(&mut state, Some(&params.text_document.uri))
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        debug!("did_close(params={params:#?})");

        let mut state = self.state.lock().await;
        state.documents.remove(&params.text_document.uri);
        self.update_presence(&mut state, None).await;
    }
}
