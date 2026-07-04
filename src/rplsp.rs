use std::collections::HashMap;
use std::sync::Mutex;

use eyre::Result;
use tokio::sync::mpsc;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

#[derive(Debug, PartialEq, Eq)]
struct Message {
    uri: Url,
    language: String,
}

#[derive(Debug)]
struct LspTask {
    tx: mpsc::Sender<Message>,
    #[expect(unused)]
    client: Client,
    documents: Mutex<HashMap<Url, DocumentDetails>>,
}

#[derive(Debug)]
struct DocumentDetails {
    language: String,
}

impl LspTask {
    async fn run(tx: mpsc::Sender<Message>) {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::new(|client| Self {
            tx,
            client,
            documents: Mutex::new(HashMap::new()),
        });
        Server::new(stdin, stdout, socket).serve(service).await;
    }

    async fn send_msg(&self, message: Message) {
        let _ = self.tx.send(message).await;
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

    async fn initialized(&self, _: InitializedParams) {}

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        {
            let mut documents = self.documents.lock().expect("what");
            documents
                .entry(params.text_document.uri.clone())
                .insert_entry(DocumentDetails {
                    language: params.text_document.language_id.clone(),
                });
        }

        self.send_msg(Message {
            uri: params.text_document.uri,
            language: params.text_document.language_id,
        })
        .await;
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp::jsonrpc::Result<Option<Hover>> {
        let r = Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String("hovering file".to_string())),
            range: None,
        }));

        let message = {
            let uri = params.text_document_position_params.text_document.uri;

            let documents = self.documents.lock().expect("what");
            let Some(details) = documents.get(&uri) else {
                return r;
            };

            Message {
                uri,
                language: details.language.clone(),
            }
        };

        self.send_msg(message).await;
        r
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let message = {
            let uri = params.text_document.uri;
            let documents = self.documents.lock().expect("what");
            let Some(details) = documents.get(&uri) else {
                return;
            };

            Message {
                uri,
                language: details.language.clone(),
            }
        };

        self.send_msg(message).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let message = {
            let uri = params.text_document.uri;
            let documents = self.documents.lock().expect("what");
            let Some(details) = documents.get(&uri) else {
                return;
            };

            Message {
                uri,
                language: details.language.clone(),
            }
        };

        self.send_msg(message).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let message = {
            let uri = params.text_document.uri;
            let mut documents = self.documents.lock().expect("what");
            let Some(details) = documents.remove(&uri) else {
                return;
            };

            Message {
                uri,
                language: details.language.clone(),
            }
        };

        self.send_msg(message).await;
    }
}

struct RpTask {
    rx: mpsc::Receiver<Message>,
}

impl RpTask {
    async fn run(rx: mpsc::Receiver<Message>) -> Result<()> {
        let mut task = Self { rx };
        tokio::spawn(async move { task.main().await })
            .await
            .expect("cannot join task")
    }

    async fn main(&mut self) -> Result<()> {
        let mut last_message = None;

        loop {
            let Some(new_message) = self.rx.recv().await else {
                return Ok(());
            };

            if last_message.as_ref().is_some_and(|x| *x == new_message) {
                continue;
            }

            last_message = Some(new_message);

            eprintln!("=> rich-presence: {last_message:#?}");
        }
    }
}

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel(25);

    let lsp_task = LspTask::run(tx);
    let rp_task = RpTask::run(rx);

    let ((), r2) = tokio::join!(lsp_task, rp_task);

    if let Err(e) = r2 {
        eprintln!("error: {e:#}");
    }
}
