use std::{collections::HashMap, process::ExitCode, sync::Mutex};

use eyre::Result;
use module::Merge;
use serde::Deserialize;
use tokio::sync::watch;
use tower_lsp::{LanguageServer, LspService, Server, lsp_types::*};

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
    let rp_task = RpTask::run(rx);

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
        _: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
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

    async fn initialized(&self, _: InitializedParams) {
        debug!("initialized");
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        debug!("shutdown");

        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        debug!("did_open");

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
        debug!("hover");

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
        debug!("did_change");

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
        debug!("did_save");

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
        debug!("did_close");

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
}

impl RpTask {
    async fn run(rx: watch::Receiver<Option<Message>>) -> Result<()> {
        let mut task = Self { rx };
        tokio::spawn(async move { task.main().await })
            .await
            .expect("cannot join task")
    }

    async fn main(&mut self) -> Result<()> {
        let mut last_message = None;

        loop {
            let _ = self.rx.changed().await;
            let Some(new_message) = self.rx.borrow_and_update().clone() else {
                continue;
            };


            if last_message.as_ref().is_some_and(|x| *x == new_message) {
                continue;
            }

            last_message = Some(new_message);

            info!("{last_message:#?}");
        }
    }
}
