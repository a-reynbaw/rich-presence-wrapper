use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::SystemTime;

use eyre::Result;
use module::Merge;
use serde::Deserialize;
use tokio::sync::watch;
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
    let (tx, rx) = watch::channel(None);

    let lsp_task = LspTask::run(tx);
    let rp_task = RpTask::run(rx, Discord::builder().client_id(CLIENT_ID).finish());

    let ((), r) = tokio::join!(lsp_task, rp_task);

    r?;
    Ok(ExitCode::SUCCESS)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    uri: Url,
    language: String,
}

#[derive(Debug)]
struct LspTask {
    tx: watch::Sender<Option<Message>>,
    documents: Mutex<HashMap<Url, DocumentDetails>>,
}

#[derive(Debug)]
struct DocumentDetails {
    language: String,
}

impl LspTask {
    async fn run(tx: watch::Sender<Option<Message>>) {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::new(|client| Self {
            tx,
            documents: Mutex::new(HashMap::new()),
        });
        Server::new(stdin, stdout, socket).serve(service).await;
    }

    fn send_msg(&self, message: Message) {
        let _ = self.tx.send(Some(message));
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for LspTask {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        debug!("initialize(params={params:#?})");

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

        let mut documents = self.documents.lock().expect("what");
        documents
            .entry(params.text_document.uri.clone())
            .insert_entry(DocumentDetails {
                language: params.text_document.language_id.clone(),
            });

        self.send_msg(Message {
            uri: params.text_document.uri,
            language: params.text_document.language_id,
        });
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp::jsonrpc::Result<Option<Hover>> {
        debug!("hover(params={params:#?})");

        let uri = params.text_document_position_params.text_document.uri;

        let documents = self.documents.lock().expect("what");
        if let Some(details) = documents.get(&uri) {
            self.send_msg(Message {
                uri,
                language: details.language.clone(),
            });
        }

        Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String("hovering file".to_string())),
            range: None,
        }))
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        debug!("did_change(params={params:#?})");

        let uri = params.text_document.uri;
        let documents = self.documents.lock().expect("what");
        let Some(details) = documents.get(&uri) else {
            return;
        };

        self.send_msg(Message {
            uri,
            language: details.language.clone(),
        });
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        debug!("did_save(params={params:#?})");

        let uri = params.text_document.uri;
        let documents = self.documents.lock().expect("what");
        let Some(details) = documents.get(&uri) else {
            return;
        };

        self.send_msg(Message {
            uri,
            language: details.language.clone(),
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        debug!("did_close(params={params:#?})");

        let uri = params.text_document.uri;
        let mut documents = self.documents.lock().expect("what");
        let Some(details) = documents.remove(&uri) else {
            return;
        };

        self.send_msg(Message {
            uri,
            language: details.language.clone(),
        });
    }
}

struct RpTask {
    rx: watch::Receiver<Option<Message>>,
    discord: Discord,
}

impl RpTask {
    async fn run(rx: watch::Receiver<Option<Message>>, discord: Discord) -> Result<()> {
        let mut task = Self { rx, discord };
        tokio::spawn(async move { task.main().await })
            .await
            .expect("cannot join task")
    }

    async fn main(&mut self) -> Result<()> {
        let mut last_message = None;
        let start = SystemTime::now();

        loop {
            let _ = self.rx.changed().await;
            let Some(new_message) = self.rx.borrow_and_update().clone() else {
                continue;
            };

            if last_message.as_ref().is_some_and(|x| *x == new_message) {
                continue;
            }
            last_message = Some(new_message);

            trace!("{last_message:#?}"); // TODO: remove me

            let mut activity = Activity::new()
                .name("todo")
                .activity_type(ActivityType::Playing)
                .status_display_type(StatusDisplayType::Name)
                .timestamps(Timestamps::new().start(start.duration_since_epoch().as_secs() as i64))
                .party(Party::new().size([1, 1]));

            activity = activity.details("details").state("state");

            self.discord.set_activity(activity).await?;
        }
    }
}
